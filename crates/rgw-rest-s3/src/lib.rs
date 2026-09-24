//! The S3 REST API: the counterpart of `rgw_rest_s3.{h,cc}` (request
//! parsing and XML rendering) and, for each op implemented, the matching
//! `RGWOp` subclass from `rgw_op.{h,cc}`.
//!
//! Path-style addressing only (`/bucket/key`); virtual-host style needs a
//! configured DNS name, as in RGW's `rgw_dns_name`.

use std::sync::Arc;

use axum::Router;
use axum::extract::Extension;
use axum::response::Response;
use axum::routing::get;
use rgw_auth::Identity;
use rgw_rest::{RequestId, s3_error_response};
use rgw_sal::Driver;
use rgw_types::RgwError;

#[derive(Clone)]
pub struct S3State {
    pub driver: Arc<dyn Driver>,
}

/// STUB: the S3 worker replaces this with the full route table.
pub fn router(state: S3State) -> Router {
    Router::new().route("/", get(list_buckets)).with_state(state)
}

async fn list_buckets(Extension(_identity): Extension<Identity>, Extension(rid): Extension<RequestId>) -> Response {
    s3_error_response(&RgwError::NotImplemented, "/", &rid.0)
}
