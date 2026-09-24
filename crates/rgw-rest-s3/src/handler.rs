//! Method and subresource dispatch: `RGWHandler_REST_Service_S3`,
//! `RGWHandler_REST_Bucket_S3` and `RGWHandler_REST_Obj_S3` (`op_get`,
//! `op_put`, ...) from `rgw_rest_s3.cc`, which pick the `RGWOp` for a
//! request. Each axum handler builds the request context, runs one op, and
//! renders its error once.

use axum::body::Body;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::response::Response;
use rgw_auth::Identity;
use rgw_rest::RequestId;
use rgw_types::RgwError;

use crate::S3State;
use crate::args::{Args, BUCKET_UNSUPPORTED, OBJECT_UNSUPPORTED};
use crate::error::{OpResult, ReqCtx, S3Error};
use crate::{bucket, list, object, service};

type HandlerResult = Result<Response, S3Error>;

/// `NotImplemented` when the query names a subresource this crate lacks.
fn reject_unsupported(args: &Args, table: &[&'static str]) -> OpResult<()> {
    match args.any_of(table) {
        Some(sub) => {
            tracing::debug!(subresource = sub, "unsupported S3 subresource");
            Err(RgwError::NotImplemented.into())
        }
        None => Ok(()),
    }
}

/// `RGWHandler_REST_Service_S3::op_get`: `RGWListBuckets`.
pub(crate) async fn service_get(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    ctx.finish(service::list_buckets(state.driver.as_ref(), &identity).await)
}

/// `RGWHandler_REST_Bucket_S3::op_get`: `?location`, `?versioning`, else
/// `RGWListBucket` V1 or V2 by `list-type`.
pub(crate) async fn bucket_get(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    Path(name): Path<String>,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    let args = Args::parse(uri.query());
    let driver = state.driver.as_ref();
    let result = async {
        if args.exists("location") {
            return bucket::get_location(driver, &identity, &name).await;
        }
        if args.exists("versioning") {
            return bucket::get_versioning(driver, &identity, &name).await;
        }
        reject_unsupported(&args, BUCKET_UNSUPPORTED)?;
        if args.get("list-type") == Some("2") {
            list::list_objects_v2(driver, &identity, &name, &args).await
        } else {
            list::list_objects_v1(driver, &identity, &name, &args).await
        }
    }
    .await;
    ctx.finish(result)
}

/// `RGWHandler_REST_Bucket_S3::op_head`: `RGWStatBucket`.
pub(crate) async fn bucket_head(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    Path(name): Path<String>,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    ctx.finish(bucket::stat_bucket(state.driver.as_ref(), &identity, &name).await)
}

/// `RGWHandler_REST_Bucket_S3::op_put`: `RGWCreateBucket`; every
/// configuration subresource (`?versioning`, `?acl`, ...) is unsupported.
pub(crate) async fn bucket_put(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    Path(name): Path<String>,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    let args = Args::parse(uri.query());
    let result = async {
        reject_unsupported(&args, BUCKET_UNSUPPORTED)?;
        if args.exists("versioning") {
            return Err(RgwError::NotImplemented.into());
        }
        bucket::create_bucket(state.driver.as_ref(), &identity, &name).await
    }
    .await;
    ctx.finish(result)
}

/// `RGWHandler_REST_Bucket_S3::op_delete`: `RGWDeleteBucket`.
pub(crate) async fn bucket_delete(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    Path(name): Path<String>,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    let args = Args::parse(uri.query());
    let result = async {
        reject_unsupported(&args, BUCKET_UNSUPPORTED)?;
        bucket::delete_bucket(state.driver.as_ref(), &identity, &name).await
    }
    .await;
    ctx.finish(result)
}

/// `RGWHandler_REST_Bucket_S3::op_post`: only `?delete`
/// (`RGWDeleteMultiObj`); browser POST uploads are unsupported.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn bucket_post(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Body,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    let args = Args::parse(uri.query());
    let result = if args.exists("delete") {
        object::delete_multi(state.driver.as_ref(), &identity, &name, &headers, body).await
    } else {
        Err(RgwError::NotImplemented.into())
    };
    ctx.finish(result)
}

/// `RGWHandler_REST_Obj_S3::op_put`: `RGWPutObj`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn object_put(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path((name, key)): Path<(String, String)>,
    body: Body,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    let args = Args::parse(uri.query());
    let result = async {
        reject_unsupported(&args, OBJECT_UNSUPPORTED)?;
        object::put_object(state.driver.as_ref(), &identity, &name, &key, &headers, body).await
    }
    .await;
    ctx.finish(result)
}

/// `RGWHandler_REST_Obj_S3::op_get`: `RGWGetObj`.
pub(crate) async fn object_get(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path((name, key)): Path<(String, String)>,
) -> HandlerResult {
    get_or_head(state, identity, rid, method, uri, headers, name, key, true).await
}

/// `RGWHandler_REST_Obj_S3::op_head`: `RGWGetObj` without data.
pub(crate) async fn object_head(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Path((name, key)): Path<(String, String)>,
) -> HandlerResult {
    get_or_head(state, identity, rid, method, uri, headers, name, key, false).await
}

#[allow(clippy::too_many_arguments)]
async fn get_or_head(
    state: S3State,
    identity: Identity,
    rid: RequestId,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    name: String,
    key: String,
    get_data: bool,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    let args = Args::parse(uri.query());
    let result = async {
        reject_unsupported(&args, OBJECT_UNSUPPORTED)?;
        object::get_object(state.driver.as_ref(), &identity, &name, &key, &headers, get_data).await
    }
    .await;
    ctx.finish(result)
}

/// `RGWHandler_REST_Obj_S3::op_delete`: `RGWDeleteObj`.
pub(crate) async fn object_delete(
    State(state): State<S3State>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
    Path((name, key)): Path<(String, String)>,
) -> HandlerResult {
    let ctx = ReqCtx::new(&method, &uri, &rid);
    let args = Args::parse(uri.query());
    let result = async {
        reject_unsupported(&args, OBJECT_UNSUPPORTED)?;
        object::delete_object(state.driver.as_ref(), &identity, &name, &key).await
    }
    .await;
    ctx.finish(result)
}

/// `RGWHandler_REST_Obj_S3::op_post`: multipart initiate/complete,
/// `?restore` and `?select` are all unsupported.
pub(crate) async fn object_post(
    Extension(rid): Extension<RequestId>,
    method: Method,
    uri: Uri,
) -> HandlerResult {
    ReqCtx::new(&method, &uri, &rid).finish(Err(RgwError::NotImplemented.into()))
}
