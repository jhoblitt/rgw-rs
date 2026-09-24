//! The S3 REST API: the counterpart of `rgw_rest_s3.{h,cc}` (request
//! parsing and XML rendering) and, for each op implemented, the matching
//! `RGWOp` subclass from `rgw_op.{h,cc}`.
//!
//! Path-style addressing only (`/bucket/key`); virtual-host style needs a
//! configured DNS name, as in RGW's `rgw_dns_name`.
//!
//! Module map, following how RGW groups the code:
//!
//! | module | RGW counterpart |
//! |---|---|
//! | `handler` | `RGWHandler_REST_{Service,Bucket,Obj}_S3::op_*`: method + subresource dispatch |
//! | `service` | `RGWListBuckets` |
//! | `bucket` | `RGWCreateBucket`, `RGWDeleteBucket`, `RGWStatBucket`, `RGWGetBucketLocation`, `RGWGetBucketVersioning` |
//! | `list` | `RGWListBucket` (V1 and V2) |
//! | `object` | `RGWPutObj`, `RGWGetObj`, `RGWDeleteObj`, `RGWDeleteMultiObj` |
//! | `perm` | `RGWOp::verify_permission` and `init_processing`'s bucket load |
//! | `range` | `RGWGetObj::parse_range` |
//! | `args` | `RGWHTTPArgs` (the query string) |
//! | `xml` | the `send_response` XML bodies |
//! | `error` | `set_req_state_err` + `dump_errno` plumbing |

mod args;
mod bucket;
mod error;
mod handler;
mod list;
mod object;
mod perm;
mod range;
mod service;
mod xml;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, put};
use rgw_sal::Driver;

pub use error::S3Error;

/// State shared by every S3 handler: the SAL driver (`rgw::sal::Driver`,
/// which RGW hangs off `req_state::driver`).
#[derive(Clone)]
pub struct S3State {
    pub driver: Arc<dyn Driver>,
}

/// The S3 route table: `RGWRESTMgr_S3::get_handler` choosing between the
/// service, bucket and object handlers by how many path segments remain.
///
/// Expects [`rgw_auth::Identity`] and [`rgw_rest::RequestId`] request
/// extensions, which the auth and request-id middleware insert.
pub fn router(state: S3State) -> Router {
    let bucket_routes = || {
        get(handler::bucket_get)
            .head(handler::bucket_head)
            .put(handler::bucket_put)
            .delete(handler::bucket_delete)
            .post(handler::bucket_post)
    };
    Router::new()
        .route("/", get(handler::service_get))
        .route("/{bucket}", bucket_routes())
        .route("/{bucket}/", bucket_routes())
        .route(
            "/{bucket}/{*key}",
            put(handler::object_put)
                .get(handler::object_get)
                .head(handler::object_head)
                .delete(handler::object_delete)
                .post(handler::object_post),
        )
        .with_state(state)
}
