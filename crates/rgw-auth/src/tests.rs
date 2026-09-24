//! The middleware end to end: requests signed with this crate's primitives,
//! run through a router with [`authenticate`] layered, against a fake driver.

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Extension;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::any;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use rgw_sal::{BucketList, ByteRange, Driver, ListParams, ListResult, ObjectBody, ObjectRead};
use rgw_types::{
    AccessKey, Attrs, BucketInfo, BucketKey, BucketStats, ObjectInfo, ObjectKey, Owner, RgwError, RgwResult,
    UserId, UserInfo,
};
use tower::ServiceExt;

use crate::sigv4::{self, CredentialScope};
use crate::{AuthConfig, AuthState, Identity, authenticate};

const AKID: &str = "AKIAIOSFODNN7EXAMPLE";
const SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
const HOST: &str = "localhost:8000";

struct FakeDriver {
    user: UserInfo,
}

impl FakeDriver {
    fn new() -> Self {
        let mut user = UserInfo::new(UserId::new("alice"), "Alice", Utc::now());
        user.add_access_key(AccessKey {
            id: AKID.to_owned(),
            key: SECRET.to_owned(),
            subuser: String::new(),
            active: true,
            create_date: Utc::now(),
        });
        Self { user }
    }
}

#[async_trait]
impl Driver for FakeDriver {
    fn name(&self) -> &'static str {
        "fake"
    }
    async fn load_user(&self, _: &UserId) -> RgwResult<UserInfo> {
        Err(RgwError::NotImplemented)
    }
    async fn load_user_by_access_key(&self, access_key: &str) -> RgwResult<UserInfo> {
        if self.user.access_keys.contains_key(access_key) { Ok(self.user.clone()) } else { Err(RgwError::NoSuchUser) }
    }
    async fn load_user_by_email(&self, _: &str) -> RgwResult<UserInfo> {
        Err(RgwError::NotImplemented)
    }
    async fn store_user(&self, _: &UserInfo, _: bool) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }
    async fn remove_user(&self, _: &UserId) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }
    async fn list_users(&self, _: &str, _: usize) -> RgwResult<Vec<UserId>> {
        Err(RgwError::NotImplemented)
    }
    async fn create_bucket(&self, _: &BucketInfo) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }
    async fn load_bucket(&self, _: &str, _: &str) -> RgwResult<BucketInfo> {
        Err(RgwError::NotImplemented)
    }
    async fn list_buckets(&self, _: Option<&UserId>, _: &str, _: usize) -> RgwResult<BucketList> {
        Err(RgwError::NotImplemented)
    }
    async fn remove_bucket(&self, _: &str, _: &str) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }
    async fn bucket_stats(&self, _: &BucketKey) -> RgwResult<BucketStats> {
        Err(RgwError::NotImplemented)
    }
    async fn list_objects(&self, _: &BucketKey, _: &ListParams) -> RgwResult<ListResult> {
        Err(RgwError::NotImplemented)
    }
    async fn head_object(&self, _: &BucketKey, _: &ObjectKey) -> RgwResult<ObjectInfo> {
        Err(RgwError::NotImplemented)
    }
    async fn get_object(&self, _: &BucketKey, _: &ObjectKey, _: Option<ByteRange>) -> RgwResult<ObjectRead> {
        Err(RgwError::NotImplemented)
    }
    async fn put_object(&self, _: &BucketKey, _: &ObjectKey, _: Owner, _: Attrs, _: ObjectBody) -> RgwResult<ObjectInfo> {
        Err(RgwError::NotImplemented)
    }
    async fn delete_object(&self, _: &BucketKey, _: &ObjectKey) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }
}

/// Echo who the request was authenticated as, the framing headers the
/// handler saw, and the body.
async fn echo(Extension(id): Extension<Identity>, headers: HeaderMap, body: Bytes) -> String {
    let who = match &id {
        Identity::Anonymous => "anonymous".to_owned(),
        Identity::User(u) => format!("{}/{}", u.info.user_id, u.access_key_id),
    };
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).unwrap_or("-").to_owned();
    format!(
        "{who}|cl={}|ce={}|dcl={}|{}",
        h("content-length"),
        h("content-encoding"),
        h("x-amz-decoded-content-length"),
        String::from_utf8_lossy(&body)
    )
}

fn app(config: AuthConfig) -> Router {
    let state = AuthState { driver: Arc::new(FakeDriver::new()), config };
    Router::new()
        .route("/", any(echo))
        .route("/{*rest}", any(echo))
        .layer(from_fn_with_state(state, authenticate))
        .layer(from_fn(rgw_rest::request_id_layer))
}

fn scope_at(t: DateTime<Utc>) -> CredentialScope {
    CredentialScope { date: t.format("%Y%m%d").to_string(), region: "us-east-1".into(), service: "s3".into() }
}

/// A request under construction; `build` signs it.
struct Signed {
    method: &'static str,
    path_and_query: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    payload_hash: String,
    time: DateTime<Utc>,
    akid: &'static str,
    secret: &'static str,
}

impl Signed {
    fn new(method: &'static str, path_and_query: &'static str, body: &[u8]) -> Self {
        Self {
            method,
            path_and_query,
            headers: Vec::new(),
            body: body.to_vec(),
            payload_hash: sigv4::sha256_hex(body),
            time: Utc::now(),
            akid: AKID,
            secret: SECRET,
        }
    }

    fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    /// Sign and return the request plus the seed signature.
    fn build_with_seed(self) -> (Request<Body>, String) {
        let amz_date = sigv4::format_amz_date(self.time);
        let scope = scope_at(self.time);
        let mut req = Request::builder()
            .method(self.method)
            .uri(self.path_and_query)
            .header("host", HOST)
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", &self.payload_hash);
        for (n, v) in &self.headers {
            req = req.header(*n, v);
        }
        let mut req = req.body(Body::from(self.body)).unwrap();

        let mut signed: Vec<String> = req.headers().keys().map(|k| k.as_str().to_owned()).collect();
        signed.sort();
        let (path, query) = self.path_and_query.split_once('?').unwrap_or((self.path_and_query, ""));
        let creq = sigv4::canonical_request(self.method, path, query, req.headers(), None, &signed, &self.payload_hash);
        let key = sigv4::signing_key(self.secret, &scope).unwrap();
        let sig = sigv4::signature(&key, &sigv4::string_to_sign(&amz_date, &scope, &creq)).unwrap();
        let authz = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={}, Signature={sig}",
            self.akid,
            signed.join(";")
        );
        req.headers_mut().insert("authorization", authz.parse().unwrap());
        (req, sig)
    }

    fn build(self) -> Request<Body> {
        self.build_with_seed().0
    }
}

async fn send(req: Request<Body>) -> (StatusCode, String) {
    send_with(AuthConfig::default(), req).await
}

async fn send_with(config: AuthConfig, req: Request<Body>) -> (StatusCode, String) {
    let resp = app(config).oneshot(req).await.unwrap();
    let status = resp.status();
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

fn assert_s3_error(status: StatusCode, body: &str, want_status: u16, code: &str) {
    assert_eq!(status.as_u16(), want_status, "{body}");
    assert!(body.starts_with("<?xml"), "{body}");
    assert!(body.contains(&format!("<Code>{code}</Code>")), "{body}");
    assert!(body.contains("<RequestId>tx"), "request id missing: {body}");
}

#[tokio::test]
async fn anonymous_passes_through() {
    let req = Request::get("/bucket/key").body(Body::from("hi")).unwrap();
    let (status, body) = send(req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("anonymous|"), "{body}");
    assert!(body.ends_with("|hi"), "{body}");
}

#[tokio::test]
async fn good_signature_yields_user() {
    let req = Signed::new("GET", "/bucket/a%20b?list-type=2&prefix=x/y", b"").header("range", "bytes=0-9").build();
    let (status, body) = send(req).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.starts_with("alice/AKIAIOSFODNN7EXAMPLE|"), "{body}");
}

#[tokio::test]
async fn put_with_payload_hash_delivers_body() {
    let (status, body) = send(Signed::new("PUT", "/bucket/key", b"Welcome to Amazon S3.").build()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.ends_with("|cl=21|ce=-|dcl=-|Welcome to Amazon S3."), "{body}");
}

#[tokio::test]
async fn unsigned_payload_is_accepted() {
    let mut s = Signed::new("PUT", "/bucket/key", b"anything");
    s.payload_hash = sigv4::UNSIGNED_PAYLOAD.to_owned();
    let (status, body) = send(s.build()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.ends_with("|anything"), "{body}");
}

#[tokio::test]
async fn wrong_secret_is_signature_mismatch() {
    let mut s = Signed::new("GET", "/bucket", b"");
    s.secret = "not-the-secret";
    let (status, body) = send(s.build()).await;
    assert_s3_error(status, &body, 403, "SignatureDoesNotMatch");
    assert!(body.contains("<Resource>/bucket</Resource>"), "{body}");
}

#[tokio::test]
async fn tampered_header_is_signature_mismatch() {
    let mut req = Signed::new("GET", "/bucket", b"").header("x-amz-meta-a", "1").build();
    req.headers_mut().insert("x-amz-meta-a", "2".parse().unwrap());
    let (status, body) = send(req).await;
    assert_s3_error(status, &body, 403, "SignatureDoesNotMatch");
}

#[tokio::test]
async fn unknown_key_is_invalid_access_key() {
    let mut s = Signed::new("GET", "/bucket", b"");
    s.akid = "AKIDUNKNOWN";
    let (status, body) = send(s.build()).await;
    assert_s3_error(status, &body, 403, "InvalidAccessKeyId");
}

#[tokio::test]
async fn skewed_date_is_rejected() {
    let mut s = Signed::new("GET", "/bucket", b"");
    s.time = Utc::now() - chrono::Duration::hours(1);
    let (status, body) = send(s.build()).await;
    assert_s3_error(status, &body, 403, "RequestTimeTooSkewed");
}

#[tokio::test]
async fn payload_hash_mismatch_is_rejected() {
    let mut s = Signed::new("PUT", "/bucket/key", b"real body");
    s.payload_hash = sigv4::sha256_hex(b"other body");
    let (status, body) = send(s.build()).await;
    assert_s3_error(status, &body, 400, "XAmzContentSHA256Mismatch");
}

#[tokio::test]
async fn missing_content_sha256_is_invalid_request() {
    let mut req = Signed::new("GET", "/bucket", b"").build();
    req.headers_mut().remove("x-amz-content-sha256");
    let (status, body) = send(req).await;
    assert_s3_error(status, &body, 400, "InvalidRequest");
}

#[tokio::test]
async fn malformed_and_unsupported_schemes() {
    let mut req = Request::get("/b").body(Body::empty()).unwrap();
    req.headers_mut().insert("authorization", "AWS4-HMAC-SHA256 Credential=x".parse().unwrap());
    let (status, body) = send(req).await;
    assert_s3_error(status, &body, 400, "AuthorizationHeaderMalformed");

    let mut req = Request::get("/b").body(Body::empty()).unwrap();
    req.headers_mut().insert("authorization", "AWS AKID:c2ln".parse().unwrap());
    let (status, body) = send(req).await;
    assert_s3_error(status, &body, 501, "NotImplemented");

    let req = Request::get("/b?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature=00").body(Body::empty()).unwrap();
    let (status, body) = send(req).await;
    assert_s3_error(status, &body, 501, "NotImplemented");
}

#[tokio::test]
async fn body_over_limit_is_entity_too_large() {
    let config = AuthConfig { max_body_bytes: 4, ..AuthConfig::default() };
    let (status, body) = send_with(config, Signed::new("PUT", "/bucket/key", b"12345").build()).await;
    assert_s3_error(status, &body, 400, "EntityTooLarge");
}

#[tokio::test]
async fn signed_aws_chunked_body_is_decoded() {
    let time = Utc::now();
    let amz_date = sigv4::format_amz_date(time);
    let scope = scope_at(time);
    let key = sigv4::signing_key(SECRET, &scope).unwrap();

    // The seed signature depends on the headers, and the body on the seed,
    // so sign once with a placeholder body and swap the real one in.
    let payload_len = 11;
    let mut s = Signed::new("PUT", "/bucket/key", b"")
        .header("content-encoding", "aws-chunked")
        .header("x-amz-decoded-content-length", payload_len.to_string());
    s.payload_hash = sigv4::STREAMING_SIGNED.to_owned();
    s.time = time;
    let (req, seed) = s.build_with_seed();
    let chunked = crate::chunked::encode_signed(&[b"hello ", b"world"], &key, &amz_date, &scope, &seed);
    let (parts, _) = req.into_parts();
    let req = Request::from_parts(parts, Body::from(chunked.clone()));

    let (status, body) = send(req).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, "alice/AKIAIOSFODNN7EXAMPLE|cl=11|ce=-|dcl=-|hello world");

    // A chunk signed under another seed breaks the chain.
    let bad = crate::chunked::encode_signed(&[b"hello world"], &key, &amz_date, &scope, &"0".repeat(64));
    let mut s = Signed::new("PUT", "/bucket/key", b"").header("content-encoding", "aws-chunked");
    s.payload_hash = sigv4::STREAMING_SIGNED.to_owned();
    s.time = time;
    let (parts, _) = s.build().into_parts();
    let (status, body) = send(Request::from_parts(parts, Body::from(bad))).await;
    assert_s3_error(status, &body, 403, "SignatureDoesNotMatch");
}

#[tokio::test]
async fn unsigned_trailer_body_is_decoded_and_checksummed() {
    use base64::Engine as _;
    let crc = base64::engine::general_purpose::STANDARD.encode(crc32fast::hash(b"hello world").to_be_bytes());
    let wire = format!("6\r\nhello \r\n5\r\nworld\r\n0\r\nx-amz-checksum-crc32:{crc}\r\n\r\n");
    let mut s = Signed::new("PUT", "/bucket/key", wire.as_bytes())
        .header("content-encoding", "gzip, aws-chunked")
        .header("x-amz-trailer", "x-amz-checksum-crc32");
    s.payload_hash = sigv4::STREAMING_UNSIGNED_TRAILER.to_owned();
    let (status, body) = send(s.build()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, "alice/AKIAIOSFODNN7EXAMPLE|cl=11|ce=gzip|dcl=-|hello world");

    let wire = "5\r\nhello\r\n0\r\nx-amz-checksum-crc32:AAAAAA==\r\n\r\n";
    let mut s = Signed::new("PUT", "/bucket/key", wire.as_bytes()).header("content-encoding", "aws-chunked");
    s.payload_hash = sigv4::STREAMING_UNSIGNED_TRAILER.to_owned();
    let (status, body) = send(s.build()).await;
    assert_s3_error(status, &body, 400, "BadDigest");
}
