//! The admin ops REST API: the counterpart of `rgw_rest_admin.h` plus the
//! `/admin/user` and `/admin/bucket` handlers that RGW keeps in
//! `driver/rados/rgw_rest_{user,bucket}.cc`. Mounted by `rgwd` under the
//! configured admin entry (`rgw_admin_entry`, default `admin`).
//!
//! Requests authenticate through the same SigV4 path as S3; each op then
//! checks a capability (`users`, `buckets`, `info`) with
//! [`Identity::check_cap`] before touching the driver. The operations
//! themselves are `rgw-admin-ops`, shared with `radosgw-admin`.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rgw_admin_ops::{self as ops, UserCreateParams, UserModifyParams, dump};
use rgw_auth::Identity;
use rgw_rest::{RequestId, admin_error_response};
use rgw_sal::Driver;
use rgw_types::{CapPerm, RgwError, RgwResult, UserId};
use serde_json::{Value, json};

/// What the admin handlers share: `RGWRESTMgr_Admin`'s view of the driver.
#[derive(Clone)]
pub struct AdminState {
    pub driver: Arc<dyn Driver>,
}

/// `RGWRESTMgr_User` and `RGWRESTMgr_Bucket` under `RGWRESTMgr_Admin`,
/// plus `RGWRESTMgr_Info`. Paths are relative to the admin entry.
pub fn router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/user",
            get(user_get).put(user_put).post(user_post).delete(user_delete).fallback(method_not_allowed),
        )
        .route(
            "/bucket",
            get(bucket_get).put(bucket_put).delete(bucket_delete).fallback(method_not_allowed),
        )
        .route("/info", get(info_get).fallback(method_not_allowed))
        .with_state(state)
}

type QueryArgs = Result<Query<HashMap<String, String>>, QueryRejection>;

/// `RESTArgs`: the query string is the admin API's parameter list.
struct Args(HashMap<String, String>);

impl Args {
    fn new(query: QueryArgs) -> RgwResult<Self> {
        query
            .map(|Query(map)| Self(map))
            .map_err(|e| RgwError::InvalidArgument(e.body_text()))
    }

    /// A sub-resource flag such as `?list` or `?key`, present with or
    /// without a value.
    fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    fn string(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }

    fn str_or_empty(&self, name: &str) -> &str {
        self.0.get(name).map_or("", String::as_str)
    }

    fn required(&self, name: &str) -> RgwResult<&str> {
        match self.0.get(name) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => Err(RgwError::InvalidArgument(format!("{name} is required"))),
        }
    }

    fn uid(&self) -> RgwResult<UserId> {
        self.required("uid").map(UserId::parse)
    }

    /// `RESTArgs::get_bool`.
    fn bool(&self, name: &str) -> RgwResult<Option<bool>> {
        self.0.get(name).map(|v| parse_bool(name, v)).transpose()
    }

    /// `RESTArgs::get_int32` / `get_uint32`.
    fn int<T: FromStr>(&self, name: &str) -> RgwResult<Option<T>> {
        self.0
            .get(name)
            .map(|v| v.parse().map_err(|_| RgwError::InvalidArgument(format!("{name}: not a number: {v}"))))
            .transpose()
    }
}

/// `RESTArgs::get_bool`'s vocabulary: `true/yes/1` and `false/no/0`.
pub fn parse_bool(name: &str, value: &str) -> RgwResult<bool> {
    match value {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        other => Err(RgwError::InvalidArgument(format!("{name}: not a boolean: {other}"))),
    }
}

/// `RGWRESTOp::check_caps` against `RGWOp::verify_permission`; anonymous
/// callers hold no caps.
fn require_cap(identity: &Identity, cap: &str, perm: CapPerm) -> RgwResult<()> {
    if identity.check_cap(cap, perm) { Ok(()) } else { Err(RgwError::AccessDenied) }
}

/// What a successful op sends: RGW's formatter output, or nothing for ops
/// that flush no section (removals).
enum Reply {
    Json(Value),
    Empty,
}

fn finish(rid: &RequestId, result: RgwResult<Reply>) -> Response {
    match result {
        Ok(Reply::Json(v)) => match serde_json::to_string_pretty(&v) {
            Ok(body) => (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], body).into_response(),
            Err(e) => admin_error_response(&RgwError::internal(e), &rid.0),
        },
        Ok(Reply::Empty) => (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")]).into_response(),
        Err(e) => admin_error_response(&e, &rid.0),
    }
}

/// Sub-resources RGW serves that this spike does not.
fn unsupported(args: &Args, names: &[&str]) -> RgwResult<()> {
    if names.iter().any(|n| args.has(n)) { Err(RgwError::NotImplemented) } else { Ok(()) }
}

/// `RGWHandler_User::op_get`: `RGWOp_User_Info` or `RGWOp_User_List`.
async fn user_get(
    State(state): State<AdminState>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    query: QueryArgs,
) -> Response {
    let result = async {
        let args = Args::new(query)?;
        unsupported(&args, &["quota"])?;
        require_cap(&identity, "users", CapPerm::READ)?;
        let driver = state.driver.as_ref();
        if args.has("list") {
            let max = args.int::<usize>("max-entries")?.unwrap_or(0);
            let list = ops::user_list(driver, args.str_or_empty("marker"), max).await?;
            return Ok(Reply::Json(dump::user_list(&list.ids, list.truncated, &list.next_marker)));
        }
        let info = ops::user_info(driver, &args.uid()?).await?;
        Ok(Reply::Json(dump::user_info(&info)))
    }
    .await;
    finish(&rid, result)
}

/// `RGWHandler_User::op_put`: `RGWOp_User_Create`, `RGWOp_Key_Create` or
/// `RGWOp_Caps_Add`.
async fn user_put(
    State(state): State<AdminState>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    query: QueryArgs,
) -> Response {
    let result = async {
        let args = Args::new(query)?;
        unsupported(&args, &["subuser", "quota"])?;
        require_cap(&identity, "users", CapPerm::WRITE)?;
        let driver = state.driver.as_ref();
        if args.has("key") {
            let uid = args.uid()?;
            let generate = args.bool("generate-key")?.unwrap_or(true);
            let info = ops::key_create(
                driver,
                &uid,
                args.0.get("access-key").map(String::as_str),
                args.0.get("secret-key").map(String::as_str),
                generate,
            )
            .await?;
            return Ok(Reply::Json(dump::keys(&info)));
        }
        if args.has("caps") {
            let info = ops::caps_add(driver, &args.uid()?, args.required("user-caps")?).await?;
            return Ok(Reply::Json(dump::caps(&info)));
        }
        let params = UserCreateParams {
            uid: args.uid()?,
            display_name: args.required("display-name")?.to_owned(),
            email: args.string("email"),
            access_key: args.string("access-key"),
            secret_key: args.string("secret-key"),
            generate_key: args.bool("generate-key")?,
            caps: args.string("user-caps"),
            max_buckets: args.int("max-buckets")?,
            suspended: args.bool("suspended")?,
            system: args.bool("system")?,
            admin: None,
        };
        let info = ops::user_create(driver, params).await?;
        Ok(Reply::Json(dump::user_info(&info)))
    }
    .await;
    finish(&rid, result)
}

/// `RGWHandler_User::op_post`: `RGWOp_User_Modify`.
async fn user_post(
    State(state): State<AdminState>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    query: QueryArgs,
) -> Response {
    let result = async {
        let args = Args::new(query)?;
        unsupported(&args, &["subuser", "quota"])?;
        require_cap(&identity, "users", CapPerm::WRITE)?;
        let params = UserModifyParams {
            uid: args.uid()?,
            display_name: args.string("display-name"),
            email: args.string("email"),
            max_buckets: args.int("max-buckets")?,
            suspended: args.bool("suspended")?,
            system: args.bool("system")?,
            admin: None,
            access_key: args.string("access-key"),
            secret_key: args.string("secret-key"),
            generate_key: args.bool("generate-key")?.unwrap_or(false),
        };
        let info = ops::user_modify(state.driver.as_ref(), params).await?;
        Ok(Reply::Json(dump::user_info(&info)))
    }
    .await;
    finish(&rid, result)
}

/// `RGWHandler_User::op_delete`: `RGWOp_User_Remove`, `RGWOp_Key_Remove`
/// or `RGWOp_Caps_Remove`.
async fn user_delete(
    State(state): State<AdminState>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    query: QueryArgs,
) -> Response {
    let result = async {
        let args = Args::new(query)?;
        unsupported(&args, &["subuser", "quota"])?;
        require_cap(&identity, "users", CapPerm::WRITE)?;
        let driver = state.driver.as_ref();
        let uid = args.uid()?;
        if args.has("key") {
            ops::key_remove(driver, &uid, args.required("access-key")?).await?;
            return Ok(Reply::Empty);
        }
        if args.has("caps") {
            let info = ops::caps_remove(driver, &uid, args.required("user-caps")?).await?;
            return Ok(Reply::Json(dump::caps(&info)));
        }
        let purge = args.bool("purge-data")?.unwrap_or(false);
        ops::user_remove(driver, &uid, purge).await?;
        Ok(Reply::Empty)
    }
    .await;
    finish(&rid, result)
}

/// `RGWHandler_Bucket::op_get`: `RGWOp_Bucket_Info`.
async fn bucket_get(
    State(state): State<AdminState>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    query: QueryArgs,
) -> Response {
    let result = async {
        let args = Args::new(query)?;
        unsupported(&args, &["index", "policy"])?;
        require_cap(&identity, "buckets", CapPerm::READ)?;
        let driver = state.driver.as_ref();
        let stats = args.bool("stats")?.unwrap_or(false);
        if let Some(bucket) = args.0.get("bucket").filter(|b| !b.is_empty()) {
            let (tenant, name) = ops::parse_bucket(bucket);
            let (info, st) = ops::bucket_info(driver, &tenant, &name).await?;
            return Ok(Reply::Json(dump::bucket_info(&info, stats.then_some(&st))));
        }
        let owner = args.0.get("uid").filter(|u| !u.is_empty()).map(|u| UserId::parse(u));
        let buckets = ops::bucket_list(driver, owner.as_ref()).await?;
        let mut out = Vec::with_capacity(buckets.len());
        for b in &buckets {
            out.push(if stats {
                let st = driver.bucket_stats(&b.bucket).await?;
                dump::bucket_info(b, Some(&st))
            } else {
                Value::String(b.bucket.name.clone())
            });
        }
        Ok(Reply::Json(Value::Array(out)))
    }
    .await;
    finish(&rid, result)
}

/// `RGWHandler_Bucket::op_put`: link and unlink, not supported here.
async fn bucket_put(Extension(rid): Extension<RequestId>) -> Response {
    finish(&rid, Err(RgwError::NotImplemented))
}

/// `RGWHandler_Bucket::op_delete`: `RGWOp_Bucket_Remove`.
async fn bucket_delete(
    State(state): State<AdminState>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
    query: QueryArgs,
) -> Response {
    let result = async {
        let args = Args::new(query)?;
        unsupported(&args, &["object"])?;
        require_cap(&identity, "buckets", CapPerm::WRITE)?;
        let (tenant, name) = ops::parse_bucket(args.required("bucket")?);
        let purge = args.bool("purge-objects")?.unwrap_or(false);
        ops::bucket_remove(state.driver.as_ref(), &tenant, &name, purge).await?;
        Ok(Reply::Empty)
    }
    .await;
    finish(&rid, result)
}

/// `RGWOp_Info_Get` (`rgw_rest_info.cc`).
async fn info_get(
    State(state): State<AdminState>,
    Extension(identity): Extension<Identity>,
    Extension(rid): Extension<RequestId>,
) -> Response {
    let result = require_cap(&identity, "info", CapPerm::READ).map(|()| {
        Reply::Json(json!({
            "info": {
                "storage_backends": [{"name": state.driver.name(), "cluster_id": "rgw-rs"}],
            }
        }))
    });
    finish(&rid, result)
}

async fn method_not_allowed(rid: Option<Extension<RequestId>>) -> Response {
    let rid = rid.map(|Extension(r)| r).unwrap_or_else(|| RequestId(String::new()));
    finish(&rid, Err(RgwError::MethodNotAllowed))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use chrono::Utc;
    use rgw_auth::AuthedUser;
    use rgw_sal::{BucketList, ByteRange, ListParams, ListResult, ObjectBody, ObjectRead};
    use rgw_types::{
        Attrs, BucketInfo, BucketKey, BucketStats, ObjectInfo, ObjectKey, Owner, UserInfo,
    };
    use tower::ServiceExt;

    use super::*;

    /// Every method fails with `NotImplemented` and counts the call, so a
    /// test can tell whether a request reached the driver.
    #[derive(Default)]
    struct FakeDriver {
        calls: AtomicUsize,
    }

    impl FakeDriver {
        fn hit<T>(&self) -> RgwResult<T> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(RgwError::NotImplemented)
        }
    }

    #[async_trait]
    impl Driver for FakeDriver {
        fn name(&self) -> &'static str {
            "fake"
        }
        async fn load_user(&self, _: &UserId) -> RgwResult<UserInfo> {
            self.hit()
        }
        async fn load_user_by_access_key(&self, _: &str) -> RgwResult<UserInfo> {
            self.hit()
        }
        async fn load_user_by_email(&self, _: &str) -> RgwResult<UserInfo> {
            self.hit()
        }
        async fn store_user(&self, _: &UserInfo, _: bool) -> RgwResult<()> {
            self.hit()
        }
        async fn remove_user(&self, _: &UserId) -> RgwResult<()> {
            self.hit()
        }
        async fn list_users(&self, _: &str, _: usize) -> RgwResult<Vec<UserId>> {
            self.hit()
        }
        async fn create_bucket(&self, _: &BucketInfo) -> RgwResult<()> {
            self.hit()
        }
        async fn load_bucket(&self, _: &str, _: &str) -> RgwResult<BucketInfo> {
            self.hit()
        }
        async fn list_buckets(&self, _: Option<&UserId>, _: &str, _: usize) -> RgwResult<BucketList> {
            self.hit()
        }
        async fn remove_bucket(&self, _: &str, _: &str) -> RgwResult<()> {
            self.hit()
        }
        async fn bucket_stats(&self, _: &BucketKey) -> RgwResult<BucketStats> {
            self.hit()
        }
        async fn list_objects(&self, _: &BucketKey, _: &ListParams) -> RgwResult<ListResult> {
            self.hit()
        }
        async fn head_object(&self, _: &BucketKey, _: &ObjectKey) -> RgwResult<ObjectInfo> {
            self.hit()
        }
        async fn get_object(&self, _: &BucketKey, _: &ObjectKey, _: Option<ByteRange>) -> RgwResult<ObjectRead> {
            self.hit()
        }
        async fn put_object(
            &self,
            _: &BucketKey,
            _: &ObjectKey,
            _: Owner,
            _: Attrs,
            _: ObjectBody,
        ) -> RgwResult<ObjectInfo> {
            self.hit()
        }
        async fn delete_object(&self, _: &BucketKey, _: &ObjectKey) -> RgwResult<()> {
            self.hit()
        }
    }

    fn user(caps: &str, admin: bool) -> Identity {
        let mut info = UserInfo::new(UserId::new("op"), "Operator", Utc::now());
        info.caps.add_from_string(caps).unwrap();
        info.admin = admin;
        Identity::User(AuthedUser { info, access_key_id: "AK".into() })
    }

    async fn call(identity: Identity, method: Method, uri: &str) -> (StatusCode, Value, usize) {
        let driver = Arc::new(FakeDriver::default());
        let app = router(AdminState { driver: driver.clone() })
            .layer(Extension(identity))
            .layer(Extension(RequestId("tx-test".into())));
        let req = Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let v = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap() };
        (status, v, driver.calls.load(Ordering::SeqCst))
    }

    #[test]
    fn bool_vocabulary() {
        for t in ["true", "yes", "1"] {
            assert!(parse_bool("x", t).unwrap());
        }
        for f in ["false", "no", "0"] {
            assert!(!parse_bool("x", f).unwrap());
        }
        for bad in ["", "TRUE", "y", "2"] {
            assert!(matches!(parse_bool("x", bad), Err(RgwError::InvalidArgument(_))), "{bad}");
        }
    }

    #[tokio::test]
    async fn missing_cap_is_403_before_driver() {
        let cases = [
            (Method::GET, "/user?uid=alice", "buckets=*"),
            (Method::GET, "/user?list", "buckets=*"),
            (Method::GET, "/user?uid=alice", "users=write"),
            (Method::PUT, "/user?uid=alice&display-name=Alice", "users=read"),
            (Method::PUT, "/user?key&uid=alice", "users=read"),
            (Method::PUT, "/user?caps&uid=alice&user-caps=usage=read", "users=read"),
            (Method::POST, "/user?uid=alice&display-name=A", "users=read"),
            (Method::DELETE, "/user?uid=alice&purge-data=true", "users=read"),
            (Method::GET, "/bucket?bucket=b1&stats=true", "users=*"),
            (Method::GET, "/bucket", "users=*"),
            (Method::DELETE, "/bucket?bucket=b1", "buckets=read"),
            (Method::GET, "/info", "users=*"),
        ];
        for (method, uri, caps) in cases {
            let (status, body, calls) = call(user(caps, false), method.clone(), uri).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
            assert_eq!(body["Code"], "AccessDenied", "{method} {uri}");
            assert_eq!(body["RequestId"], "tx-test");
            assert_eq!(calls, 0, "{method} {uri} reached the driver");
        }
    }

    #[tokio::test]
    async fn anonymous_is_403() {
        let (status, body, calls) = call(Identity::Anonymous, Method::GET, "/user?uid=alice").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["Code"], "AccessDenied");
        assert_eq!(calls, 0);
    }

    #[tokio::test]
    async fn held_cap_reaches_driver() {
        let (status, body, calls) = call(user("users=read", false), Method::GET, "/user?uid=alice").await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["Code"], "NotImplemented");
        assert_eq!(calls, 1);
        let (_, _, calls) = call(user("", true), Method::DELETE, "/bucket?bucket=b1").await;
        assert_eq!(calls, 1, "the admin flag passes every cap check");
    }

    #[tokio::test]
    async fn missing_or_malformed_params_are_invalid_argument() {
        for (method, uri) in [
            (Method::GET, "/user"),
            (Method::PUT, "/user?uid=alice"),
            (Method::PUT, "/user?caps&uid=alice"),
            (Method::DELETE, "/user?key&uid=alice"),
            (Method::DELETE, "/bucket"),
            (Method::DELETE, "/user?uid=alice&purge-data=maybe"),
            (Method::GET, "/user?list&max-entries=lots"),
        ] {
            let (status, body, calls) = call(user("users=*;buckets=*", false), method.clone(), uri).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
            assert_eq!(body["Code"], "InvalidArgument", "{method} {uri}");
            assert_eq!(calls, 0);
        }
    }

    #[tokio::test]
    async fn unsupported_subresources_are_501() {
        for (method, uri) in [
            (Method::GET, "/user?quota&uid=alice"),
            (Method::PUT, "/user?subuser&uid=alice"),
            (Method::PUT, "/user?quota&uid=alice"),
            (Method::GET, "/bucket?index&bucket=b1"),
            (Method::GET, "/bucket?policy&bucket=b1"),
            (Method::PUT, "/bucket?bucket=b1&uid=alice"),
        ] {
            let (status, body, _) = call(user("", true), method.clone(), uri).await;
            assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{method} {uri}");
            assert_eq!(body["Code"], "NotImplemented");
        }
    }

    #[tokio::test]
    async fn unrouted_method_is_405_json() {
        let (status, body, _) = call(user("", true), Method::POST, "/bucket").await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(body["Code"], "MethodNotAllowed");
    }

    #[tokio::test]
    async fn info_names_the_driver() {
        let (status, body, _) = call(user("info=read", false), Method::GET, "/info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"info": {"storage_backends": [{"name": "fake", "cluster_id": "rgw-rs"}]}}));
    }
}
