//! Shared REST plumbing: the counterpart of `rgw_rest.{h,cc}`. Error
//! rendering for both wire formats, request ids, and the date formats the
//! protocols use.

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use rgw_types::RgwError;
use serde::Serialize;

/// What RGW puts in `HostId`; RGW uses the zone plus zonegroup name.
pub const HOST_ID: &str = "rgw-rs-default-default";

/// Per-request id, inserted as a request extension by [`request_id_layer`]
/// and echoed as `x-amz-request-id`.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// RGW's ids look like `tx00000...-<hex time>-<zone>`; the shape is kept so
/// log grep habits carry over.
pub fn new_request_id() -> String {
    let id = uuid::Uuid::new_v4().as_simple().to_string();
    format!("tx{}-{:08x}-rgw-rs", &id[..16], Utc::now().timestamp() as u32)
}

/// Outermost middleware: mints the request id, exposes it to handlers, and
/// stamps `x-amz-request-id` and `Server` on every response.
pub async fn request_id_layer(mut req: Request, next: Next) -> Response {
    let id = new_request_id();
    req.extensions_mut().insert(RequestId(id.clone()));
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&id) {
        headers.insert("x-amz-request-id", v);
    }
    headers.insert(header::SERVER, HeaderValue::from_static("rgw-rs"));
    resp
}

/// `<Error>` as `rgw_rest.cc` emits it for S3 (`dump_errno` plus
/// `end_header`'s XML wrapping).
#[derive(Serialize)]
#[serde(rename = "Error")]
struct S3ErrorBody<'a> {
    #[serde(rename = "Code")]
    code: &'a str,
    #[serde(rename = "Message")]
    message: String,
    #[serde(rename = "Resource")]
    resource: &'a str,
    #[serde(rename = "RequestId")]
    request_id: &'a str,
    #[serde(rename = "HostId")]
    host_id: &'a str,
}

/// The S3 error document.
pub fn s3_error_response(err: &RgwError, resource: &str, request_id: &str) -> Response {
    let body = S3ErrorBody {
        code: err.code(),
        message: err.to_string(),
        resource,
        request_id,
        host_id: HOST_ID,
    };
    let xml = quick_xml::se::to_string(&body).unwrap_or_default();
    respond(err, "application/xml", format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{xml}"))
}

/// The admin API error, as RGW's JSON formatter renders `dump_errno`:
/// `{"Code":"NoSuchUser","RequestId":"...","HostId":"..."}`.
pub fn admin_error_response(err: &RgwError, request_id: &str) -> Response {
    let json = serde_json::json!({
        "Code": err.code(),
        "RequestId": request_id,
        "HostId": HOST_ID,
    });
    respond(err, "application/json", json.to_string())
}

fn respond(err: &RgwError, content_type: &'static str, body: String) -> Response {
    let status = StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, [(header::CONTENT_TYPE, content_type)], body).into_response()
}

/// RFC 7231 `Date` / `Last-Modified`: `Tue, 15 Nov 1994 08:12:31 GMT`.
pub fn http_date(t: DateTime<Utc>) -> String {
    t.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// S3 XML timestamps (`dump_time` in `rgw_rest.cc`): `2009-10-12T17:50:30.000Z`.
pub fn iso8601_millis(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Admin API timestamps (`utime_t::gmtime`): `2024-01-02T03:04:05.123456Z`.
pub fn iso8601_micros(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn s3_error_is_rgw_shaped() {
        let resp = s3_error_response(&RgwError::NoSuchBucket, "/b", "tx1");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error>"), "{body}");
        assert!(body.contains("<Code>NoSuchBucket</Code>"), "{body}");
        assert!(body.contains("<RequestId>tx1</RequestId>"), "{body}");
    }

    #[tokio::test]
    async fn admin_error_is_json() {
        let resp = admin_error_response(&RgwError::NoSuchUser, "tx2");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["Code"], "NoSuchUser");
        assert_eq!(v["RequestId"], "tx2");
    }
}
