//! Test harness: hosts `rgwd`'s router in-process on an ephemeral port so
//! the integration tests under `tests/` can drive it with a real S3 SDK
//! and hand-signed admin requests.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Context as _;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::{BehaviorVersion, Region};
pub use aws_sigv4::http_request::SignableBody;
use aws_sigv4::http_request::{
    PayloadChecksumKind, PercentEncodingMode, SignableRequest, SigningSettings, UriPathNormalizationMode, sign,
};
use aws_sigv4::sign::v4::SigningParams;
use rgw_admin_ops::UserCreateParams;
use rgw_sal::Driver;
use rgw_sal_sqlite::SqliteDriver;
use rgw_types::UserId;
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// The uid `TestServer::spawn` seeds with every admin cap.
pub const ADMIN_UID: &str = "admin";
/// The caps the seeded admin holds.
pub const ADMIN_CAPS: &str = "users=*;buckets=*;info=*";
/// The region every client signs for; RGW accepts any region name.
pub const REGION: &str = "default";

/// An `rgwd` router served on `127.0.0.1:<ephemeral>` over a fresh SQLite
/// database. Dropping it stops the server and deletes the database.
pub struct TestServer {
    addr: SocketAddr,
    driver: Arc<dyn Driver>,
    /// The seeded admin's `(access_key, secret_key)`.
    pub admin_keys: (String, String),
    server: JoinHandle<()>,
    // Held for its Drop; the SQLite file lives inside.
    _dir: TempDir,
}

impl TestServer {
    /// Open a new database, seed the admin user and start serving.
    pub async fn spawn() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir().context("creating the database directory")?;
        let driver: Arc<dyn Driver> = Arc::new(SqliteDriver::open(&dir.path().join("rgw.db"))?);
        let app = rgwd::build_app(driver.clone(), rgwd::DEFAULT_ADMIN_ENTRY);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("rgwd test server stopped: {e}");
            }
        });
        let mut this = Self { addr, driver, admin_keys: Default::default(), server, _dir: dir };
        this.admin_keys = this.create_user(ADMIN_UID, ADMIN_CAPS).await?;
        Ok(this)
    }

    /// `http://127.0.0.1:<port>`.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The driver the server runs on, for inspecting or seeding state
    /// behind the API's back.
    pub fn driver(&self) -> &Arc<dyn Driver> {
        &self.driver
    }

    /// Create `uid` (display name = uid) with `caps` (empty for none) and
    /// one generated S3 key; returns `(access_key, secret_key)`.
    pub async fn create_user(&self, uid: &str, caps: &str) -> anyhow::Result<(String, String)> {
        let params = UserCreateParams {
            uid: UserId::parse(uid),
            display_name: uid.to_owned(),
            generate_key: Some(true),
            caps: (!caps.is_empty()).then(|| caps.to_owned()),
            ..Default::default()
        };
        let info = rgw_admin_ops::user_create(self.driver.as_ref(), params).await?;
        let key = info.access_keys.values().next().context("user_create generated no key")?;
        Ok((key.id.clone(), key.key.clone()))
    }

    /// An S3 client pointed at this server, path-style, signing as the
    /// given key pair.
    pub fn s3_client_for(&self, access_key: &str, secret_key: &str) -> aws_sdk_s3::Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(self.endpoint())
            .region(Region::new(REGION))
            .credentials_provider(Credentials::from_keys(access_key, secret_key, None))
            .force_path_style(true)
            .build();
        aws_sdk_s3::Client::from_conf(config)
    }

    /// An S3 client signing as the seeded admin.
    pub fn admin_s3_client(&self) -> aws_sdk_s3::Client {
        self.s3_client_for(&self.admin_keys.0, &self.admin_keys.1)
    }
}

impl TestServer {
    /// Send `method <path_and_query>` with `reqwest`, SigV4-signed as S3
    /// clients sign (header auth, `x-amz-content-sha256` set, no path
    /// normalisation) when `keys` is given, unsigned otherwise.
    /// `path_and_query` must already be percent-encoded; see
    /// [`query_encode`]. `headers` are sent but not signed.
    pub async fn request(
        &self,
        method: &str,
        path_and_query: &str,
        keys: Option<(&str, &str)>,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> anyhow::Result<reqwest::Response> {
        let url = format!("{}{path_and_query}", self.endpoint());
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut builder = reqwest::Client::new().request(method.clone(), &url);
        if let Some((access_key, secret_key)) = keys {
            for (name, value) in sigv4_headers(method.as_str(), &url, access_key, secret_key, SignableBody::Bytes(&body))? {
                builder = builder.header(name, value);
            }
        }
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        Ok(builder.body(body).send().await?)
    }

    /// [`Self::request`] signed as the seeded admin.
    pub async fn admin_request(&self, method: &str, path_and_query: &str) -> anyhow::Result<reqwest::Response> {
        let (a, s) = self.admin_keys.clone();
        self.request(method, path_and_query, Some((&a, &s)), &[], Vec::new()).await
    }
}

/// The headers SigV4 adds to sign `method url` as S3 clients do (header
/// auth, `x-amz-content-sha256` set to `body`'s hash or literal, no path
/// normalisation). Only `host` and the added `x-amz-*` headers are signed.
pub fn sigv4_headers(
    method: &str,
    url: &str,
    access_key: &str,
    secret_key: &str,
    body: SignableBody<'_>,
) -> anyhow::Result<Vec<(String, String)>> {
    let identity = Credentials::from_keys(access_key, secret_key, None).into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    settings.percent_encoding_mode = PercentEncodingMode::Single;
    settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name("s3")
        .time(SystemTime::now())
        .settings(settings)
        .build()?
        .into();
    let signable = SignableRequest::new(method, url, std::iter::empty(), body)?;
    let (instructions, _signature) = sign(signable, &params)?.into_parts();
    Ok(instructions.headers().map(|(n, v)| (n.to_owned(), v.to_owned())).collect())
}

/// Percent-encode a query-string component with the SigV4 unreserved set,
/// so what is sent is already canonical.
pub fn query_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.server.abort();
    }
}
