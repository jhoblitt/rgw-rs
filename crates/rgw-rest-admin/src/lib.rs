//! The admin ops REST API: the counterpart of `rgw_rest_admin.h` plus the
//! `/admin/user` and `/admin/bucket` handlers that RGW keeps in
//! `driver/rados/rgw_rest_{user,bucket}.cc`. Mounted by `rgwd` under the
//! configured admin entry (`rgw_admin_entry`, default `admin`).
//!
//! Requests authenticate through the same SigV4 path as S3; each op then
//! checks a capability (`users`, `buckets`) with [`Identity::check_cap`].

use std::sync::Arc;

use axum::Router;
use axum::extract::Extension;
use axum::response::Response;
use axum::routing::get;
use rgw_auth::Identity;
use rgw_rest::{RequestId, admin_error_response};
use rgw_sal::Driver;
use rgw_types::RgwError;

#[derive(Clone)]
pub struct AdminState {
    pub driver: Arc<dyn Driver>,
}

/// STUB: the admin worker replaces this with the full route table.
pub fn router(state: AdminState) -> Router {
    Router::new()
        .route("/user", get(not_implemented))
        .route("/bucket", get(not_implemented))
        .with_state(state)
}

async fn not_implemented(Extension(_identity): Extension<Identity>, Extension(rid): Extension<RequestId>) -> Response {
    admin_error_response(&RgwError::NotImplemented, &rid.0)
}
