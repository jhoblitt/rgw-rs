//! Error plumbing: the counterpart of `set_req_state_err` + `dump_errno` +
//! `end_header` in `rgw_rest.cc`, which turn an op's negative return code
//! into the S3 error document once, after the op has run.

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rgw_rest::{RequestId, s3_error_response};
use rgw_types::RgwError;

/// An op's failure: the error plus any headers the reply must still carry
/// (`Content-Range: bytes */<size>` on an unsatisfiable range).
#[derive(Debug)]
pub(crate) struct OpError {
    pub(crate) err: RgwError,
    pub(crate) headers: HeaderMap,
}

impl From<RgwError> for OpError {
    fn from(err: RgwError) -> Self {
        Self { err, headers: HeaderMap::new() }
    }
}

pub(crate) type OpResult<T> = Result<T, OpError>;

/// What a handler needs to render an error: `req_state`'s request URI,
/// transaction id, and whether the method permits a body.
pub(crate) struct ReqCtx {
    resource: String,
    request_id: String,
    head: bool,
}

impl ReqCtx {
    pub(crate) fn new(method: &Method, uri: &Uri, request_id: &RequestId) -> Self {
        Self {
            resource: uri.path().to_owned(),
            request_id: request_id.0.clone(),
            head: method == Method::HEAD,
        }
    }

    /// Attach the request context to an op's outcome; the one place an
    /// `OpError` becomes a rendered error.
    pub(crate) fn finish(self, result: OpResult<Response>) -> Result<Response, S3Error> {
        result.map_err(|op| S3Error {
            err: op.err,
            headers: op.headers,
            resource: self.resource,
            request_id: self.request_id,
            head: self.head,
        })
    }
}

/// A failed S3 request, rendered as the S3 `<Error>` document, or as the
/// bare status for HEAD, which has no body (`RGWGetObj` / `RGWStatBucket`
/// with `get_data=false`).
#[derive(Debug)]
pub struct S3Error {
    err: RgwError,
    headers: HeaderMap,
    resource: String,
    request_id: String,
    head: bool,
}

impl IntoResponse for S3Error {
    fn into_response(self) -> Response {
        tracing::debug!(code = self.err.code(), resource = %self.resource, "s3 request failed: {}", self.err);
        let mut resp = if self.head {
            let status = StatusCode::from_u16(self.err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            status.into_response()
        } else {
            s3_error_response(&self.err, &self.resource, &self.request_id)
        };
        resp.headers_mut().extend(self.headers);
        resp
    }
}

/// A header value built from data RGW formatted itself; a failure means an
/// unrepresentable byte slipped in, which is a server-side bug.
pub(crate) fn header_value(s: &str) -> Result<HeaderValue, RgwError> {
    HeaderValue::from_str(s).map_err(RgwError::internal)
}
