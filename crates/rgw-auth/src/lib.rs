//! Request authentication: the counterpart of `rgw_auth_s3.{h,cc}` and the
//! `rgw::auth` engines in `rgw_auth.h`, trimmed to AWS Signature Version 4
//! with header-based signing (presigned URLs and SigV2 are out of scope)
//! plus anonymous access.
//!
//! [`authenticate`] is an axum middleware. It verifies the signature and the
//! payload hash, decodes `aws-chunked` bodies so handlers see plain bytes,
//! and inserts an [`Identity`] request extension. Failures are rendered as
//! S3 error documents, as RGW does for every API behind the S3 auth path.

pub mod chunked;
pub mod sigv4;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{OriginalUri, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use bytes::Bytes;
use chrono::Utc;
use rgw_sal::Driver;
use rgw_types::{CapPerm, RgwError, RgwResult, UserId, UserInfo};

use crate::chunked::{ChunkSigner, decode_aws_chunked, verify_trailer_checksum};
use crate::sigv4::{AuthorizationV4, CredentialScope};

/// A request signed by a known access key.
#[derive(Clone, Debug)]
pub struct AuthedUser {
    pub info: UserInfo,
    pub access_key_id: String,
}

/// `rgw::auth::Identity`, trimmed to the two kinds this spike serves.
#[derive(Clone, Debug)]
pub enum Identity {
    Anonymous,
    User(AuthedUser),
}

impl Identity {
    pub fn user(&self) -> Option<&UserInfo> {
        match self {
            Self::Anonymous => None,
            Self::User(u) => Some(&u.info),
        }
    }

    pub fn user_id(&self) -> Option<&UserId> {
        self.user().map(|u| &u.user_id)
    }

    /// `rgw::auth::Identity::is_admin_of` for the admin API: the `admin`
    /// flag short-circuits, otherwise the cap must be held.
    pub fn check_cap(&self, cap: &str, perm: CapPerm) -> bool {
        self.user().is_some_and(|u| u.admin || u.caps.check_cap(cap, perm))
    }
}

/// The `rgw_auth_*` / `rgw_max_put_size` knobs the middleware consults.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// Allowed clock skew, `rgw_auth_max_skew` (RGW's default is 15 minutes).
    pub max_skew: chrono::Duration,
    /// Largest request body the middleware will buffer, before any
    /// `aws-chunked` decoding. RGW streams instead; its nearest limit is
    /// `rgw_max_put_size` (5 GiB).
    pub max_body_bytes: usize,
}

/// The default [`AuthConfig::max_body_bytes`].
pub const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

impl Default for AuthConfig {
    fn default() -> Self {
        Self { max_skew: chrono::Duration::minutes(15), max_body_bytes: DEFAULT_MAX_BODY_BYTES }
    }
}

#[derive(Clone)]
pub struct AuthState {
    pub driver: Arc<dyn Driver>,
    pub config: AuthConfig,
}

/// The S3 auth strategy (`rgw::auth::s3::AWSAuthStrategy` driving
/// `LocalEngine` over `AWSGeneralAbstractor`), as axum middleware.
///
/// Inserts [`Identity::Anonymous`] for an unsigned request and
/// [`Identity::User`] for a valid SigV4 one, replacing the body with the
/// verified, decoded payload. Anything else ends the request with an S3
/// error document.
pub async fn authenticate(State(state): State<AuthState>, req: Request, next: Next) -> Response {
    let request_id = req.extensions().get::<rgw_rest::RequestId>().map(|r| r.0.clone()).unwrap_or_default();
    let resource = original_uri(&req).path().to_owned();
    match verify(&state, req).await {
        Ok(req) => next.run(req).await,
        Err(err) => {
            tracing::debug!(%request_id, %resource, error = %err, "authentication failed");
            rgw_rest::s3_error_response(&err, &resource, &request_id)
        }
    }
}

/// The URI the client sent, which is what it signed; a nested router sees
/// a prefix-stripped one.
fn original_uri(req: &Request) -> &axum::http::Uri {
    req.extensions().get::<OriginalUri>().map_or(req.uri(), |u| &u.0)
}

fn has_query_param(query: &str, name: &str) -> bool {
    query.split('&').any(|p| {
        let key = p.split_once('=').map_or(p, |(k, _)| k);
        percent_encoding::percent_decode_str(key).decode_utf8_lossy() == name
    })
}

fn header_str<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

async fn verify(state: &AuthState, req: Request) -> RgwResult<Request> {
    let uri = original_uri(&req).clone();
    let query = uri.query().unwrap_or("");
    let (mut parts, body) = req.into_parts();

    if has_query_param(query, "X-Amz-Algorithm") {
        return Err(RgwError::NotImplemented);
    }
    let Some(authorization) = parts.headers.get(header::AUTHORIZATION) else {
        parts.extensions.insert(Identity::Anonymous);
        return Ok(Request::from_parts(parts, body));
    };
    let authorization = authorization
        .to_str()
        .map_err(|_| RgwError::AuthorizationHeaderMalformed("not ASCII".to_owned()))?;
    let auth = match authorization.split_once([' ', '\t']).map_or(authorization, |(scheme, _)| scheme) {
        sigv4::ALGORITHM => sigv4::parse_authorization(authorization)?,
        // SigV2 and SigV4A.
        "AWS" | "AWS4-ECDSA-P256-SHA256" => return Err(RgwError::NotImplemented),
        _ => return Err(RgwError::AuthorizationHeaderMalformed("unsupported authorization scheme".to_owned())),
    };

    let amz_date = request_time(&parts.headers, &auth.scope, state.config.max_skew)?;

    let info = match state.driver.load_user_by_access_key(&auth.access_key_id).await {
        Ok(info) => info,
        Err(RgwError::NoSuchUser) => return Err(RgwError::InvalidAccessKeyId),
        Err(e) => return Err(e),
    };
    if info.suspended {
        return Err(RgwError::UserSuspended);
    }
    let secret = match info.access_keys.get(&auth.access_key_id) {
        Some(key) if key.active => key.key.clone(),
        _ => return Err(RgwError::InvalidAccessKeyId),
    };

    let payload_hash = header_str(&parts.headers, "x-amz-content-sha256")
        .ok_or_else(|| RgwError::InvalidRequest("missing x-amz-content-sha256".to_owned()))?
        .to_owned();
    let creq = sigv4::canonical_request(
        parts.method.as_str(),
        uri.path(),
        query,
        &parts.headers,
        parts.uri.authority().map(|a| a.as_str()),
        &auth.signed_headers,
        &payload_hash,
    );
    let signing_key = sigv4::signing_key(&secret, &auth.scope)?;
    let computed = sigv4::signature_bytes(&signing_key, &sigv4::string_to_sign(&amz_date, &auth.scope, &creq))?;
    if !sigv4::signature_matches(&computed, &auth.signature) {
        tracing::debug!(canonical_request = %creq, "signature mismatch");
        return Err(RgwError::SignatureDoesNotMatch);
    }

    let raw = read_body(&parts.headers, body, state.config.max_body_bytes).await?;
    let payload = verify_payload(&mut parts, raw, &payload_hash, &auth, &amz_date, &signing_key)?;

    parts.headers.remove(header::TRANSFER_ENCODING);
    parts.headers.insert(header::CONTENT_LENGTH, HeaderValue::from(payload.len()));
    parts.extensions.insert(Identity::User(AuthedUser { info, access_key_id: auth.access_key_id }));
    Ok(Request::from_parts(parts, Body::from(payload)))
}

/// Pick the signing time (`x-amz-date`, else `Date`), check it against the
/// credential scope and the allowed skew, and return it in the `x-amz-date`
/// form the string to sign uses.
fn request_time(headers: &HeaderMap, scope: &CredentialScope, max_skew: chrono::Duration) -> RgwResult<String> {
    let missing = || RgwError::AccessDenied;
    let t = match header_str(headers, "x-amz-date") {
        Some(v) => sigv4::parse_amz_date(v).ok_or_else(missing)?,
        None => header_str(headers, "date").and_then(sigv4::parse_http_date).ok_or_else(missing)?,
    };
    let amz_date = sigv4::format_amz_date(t);
    if !amz_date.starts_with(&scope.date) {
        return Err(RgwError::AuthorizationHeaderMalformed(
            "credential date does not match the request date".to_owned(),
        ));
    }
    if (Utc::now() - t).abs() > max_skew {
        return Err(RgwError::RequestTimeTooSkewed);
    }
    Ok(amz_date)
}

async fn read_body(headers: &HeaderMap, body: Body, limit: usize) -> RgwResult<Bytes> {
    let declared = header_str(headers, "content-length").and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > limit as u64) {
        return Err(RgwError::EntityTooLarge);
    }
    axum::body::to_bytes(body, limit).await.map_err(|e| {
        let e = e.into_inner();
        if e.is::<http_body_util::LengthLimitError>() {
            RgwError::EntityTooLarge
        } else {
            RgwError::InvalidRequest(format!("reading the request body: {e}"))
        }
    })
}

/// Check the buffered body against `x-amz-content-sha256` and return the
/// payload handlers should see, fixing up the framing headers when the body
/// was `aws-chunked`.
fn verify_payload(
    parts: &mut Parts,
    raw: Bytes,
    payload_hash: &str,
    auth: &AuthorizationV4,
    amz_date: &str,
    signing_key: &[u8; 32],
) -> RgwResult<Bytes> {
    let signer = ChunkSigner { signing_key, amz_date, scope: &auth.scope, seed_signature: &auth.signature };
    let decoded = match payload_hash {
        sigv4::UNSIGNED_PAYLOAD => return Ok(raw),
        sigv4::STREAMING_SIGNED => decode_aws_chunked(&raw, Some(&signer), false)?,
        sigv4::STREAMING_SIGNED_TRAILER => decode_aws_chunked(&raw, Some(&signer), true)?,
        sigv4::STREAMING_UNSIGNED_TRAILER => decode_aws_chunked(&raw, None, true)?,
        h if h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()) => {
            if !sigv4::sha256_hex(&raw).eq_ignore_ascii_case(h) {
                return Err(RgwError::XAmzContentSha256Mismatch);
            }
            return Ok(raw);
        }
        h if h.starts_with("STREAMING-") => return Err(RgwError::NotImplemented),
        _ => return Err(RgwError::InvalidRequest("invalid x-amz-content-sha256".to_owned())),
    };
    verify_trailer_checksum(&decoded)?;

    let headers = &mut parts.headers;
    if let Some(declared) = header_str(headers, "x-amz-decoded-content-length") {
        if declared.parse::<usize>().ok() != Some(decoded.payload.len()) {
            return Err(RgwError::InvalidRequest("x-amz-decoded-content-length does not match the payload".to_owned()));
        }
        headers.remove("x-amz-decoded-content-length");
    }
    strip_aws_chunked_encoding(headers);
    Ok(Bytes::from(decoded.payload))
}

/// Drop the `aws-chunked` token from `Content-Encoding`, keeping any real
/// encoding (e.g. `gzip`) the client layered under it.
fn strip_aws_chunked_encoding(headers: &mut HeaderMap) {
    let Some(value) = header_str(headers, "content-encoding") else {
        return;
    };
    let rest: Vec<&str> =
        value.split(',').map(str::trim).filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("aws-chunked")).collect();
    if rest.is_empty() {
        headers.remove(header::CONTENT_ENCODING);
    } else if let Ok(v) = HeaderValue::from_str(&rest.join(", ")) {
        headers.insert(header::CONTENT_ENCODING, v);
    }
}
