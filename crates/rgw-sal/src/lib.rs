//! The storage abstraction layer: the Rust counterpart of `rgw::sal` in
//! `src/rgw/rgw_sal.h`.
//!
//! RGW's SAL is a hierarchy of handle objects (`Driver` -> `User`, `Bucket`,
//! `Object`, `Writer`, ...) that each carry cached state plus `load_*` /
//! `store_*` methods: 404 pure-virtual methods across the hierarchy. Here it
//! collapses into one object-safe trait of async operations over plain
//! values. The handles exist to carry state down a synchronous C++ call
//! chain; a Rust request holds its values directly. Names are kept from the
//! C++ (`load_user`, `store_user`, `list_buckets`, `ListParams`) so a reader
//! can cross-reference.
//!
//! Every method documents its error contract and [`testsuite`] checks a
//! driver against it.

pub mod testsuite;

use std::io;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use rgw_types::{
    Attrs, BucketInfo, BucketKey, BucketStats, ObjectInfo, ObjectKey, Owner, RgwResult, UserId,
    UserInfo,
};

/// Object payload as a stream of chunks: the counterpart of RGW's
/// `RGWGetDataCB` / `DataProcessor` chunk callbacks.
pub type ObjectBody = BoxStream<'static, io::Result<Bytes>>;

/// Page size when a caller passes 0 for `max`.
pub const DEFAULT_MAX_ENTRIES: usize = 1000;

/// `rgw::sal::Bucket::ListParams`, trimmed to what S3 ListObjects(V2) needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListParams {
    pub prefix: String,
    /// Empty for no delimiter.
    pub delimiter: String,
    /// Return entries strictly greater than this (S3 `marker` / `start-after`).
    pub marker: String,
    /// Upper bound on `objects.len() + common_prefixes.len()`; 0 means
    /// [`DEFAULT_MAX_ENTRIES`].
    pub max_keys: usize,
}

/// `rgw::sal::Bucket::ListResults`.
#[derive(Debug, Default)]
pub struct ListResult {
    pub objects: Vec<ObjectInfo>,
    pub common_prefixes: Vec<String>,
    pub is_truncated: bool,
    /// The last entry (key or common prefix) returned, for the next `marker`.
    pub next_marker: String,
}

/// `rgw::sal::BucketList`.
#[derive(Debug, Default)]
pub struct BucketList {
    pub buckets: Vec<BucketInfo>,
    pub is_truncated: bool,
    pub next_marker: String,
}

/// A requested byte range, from HTTP `Range: bytes=...`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteRange {
    /// `bytes=start-end` (inclusive), or `bytes=start-` when `end` is `None`.
    Absolute { start: u64, end: Option<u64> },
    /// `bytes=-len`: the last `len` bytes.
    Suffix(u64),
}

/// The result of [`Driver::get_object`].
pub struct ObjectRead {
    pub info: ObjectInfo,
    /// The resolved inclusive range when one was requested; `body` then
    /// carries only those bytes.
    pub range: Option<(u64, u64)>,
    pub body: ObjectBody,
}

/// `rgw::sal::Driver`, `User`, `Bucket` and `Object` flattened into one
/// trait. Implementations are shared across requests as `Arc<dyn Driver>`.
///
/// Buckets are identified by `(tenant, name)` at the API and by
/// `BucketKey::bucket_id` once loaded, as in RGW where the entrypoint object
/// maps a name to a bucket instance.
#[async_trait]
pub trait Driver: Send + Sync + 'static {
    /// `Driver::get_name()`.
    fn name(&self) -> &'static str;

    // ---- users: `rgw::sal::User` ------------------------------------------

    /// `User::load_user`. Errors: `NoSuchUser`.
    async fn load_user(&self, id: &UserId) -> RgwResult<UserInfo>;

    /// `Driver::get_user_by_access_key`. Errors: `NoSuchUser`.
    async fn load_user_by_access_key(&self, access_key: &str) -> RgwResult<UserInfo>;

    /// `Driver::get_user_by_email`. Errors: `NoSuchUser`.
    async fn load_user_by_email(&self, email: &str) -> RgwResult<UserInfo>;

    /// `User::store_user`. Replaces the whole record, including the access
    /// key index, preserving timestamps to at least whole seconds. Errors:
    /// `UserAlreadyExists` when `exclusive` and the id exists; `KeyExists`
    /// when an access key id belongs to another user.
    async fn store_user(&self, info: &UserInfo, exclusive: bool) -> RgwResult<()>;

    /// `User::remove_user`. Buckets the user owns are left alone; purging
    /// is an admin-op concern. Errors: `NoSuchUser`.
    async fn remove_user(&self, id: &UserId) -> RgwResult<()>;

    /// `Driver::meta_list_keys_*` over users: ids (in `tenant$id` form)
    /// strictly greater than `marker`, ascending, at most `max` (0 means
    /// [`DEFAULT_MAX_ENTRIES`]).
    async fn list_users(&self, marker: &str, max: usize) -> RgwResult<Vec<UserId>>;

    // ---- buckets: `rgw::sal::Bucket` --------------------------------------

    /// `Bucket::create`. Errors: `BucketAlreadyExists`.
    async fn create_bucket(&self, info: &BucketInfo) -> RgwResult<()>;

    /// `Driver::load_bucket`. Errors: `NoSuchBucket`.
    async fn load_bucket(&self, tenant: &str, name: &str) -> RgwResult<BucketInfo>;

    /// `Driver::list_buckets`: by name ascending, strictly after `marker`,
    /// at most `max` (0 means the default). `owner` of `None` lists every
    /// bucket, which is what `/admin/bucket` without a `uid` does.
    async fn list_buckets(
        &self,
        owner: Option<&UserId>,
        marker: &str,
        max: usize,
    ) -> RgwResult<BucketList>;

    /// `Bucket::remove`. Errors: `NoSuchBucket`, `BucketNotEmpty`.
    async fn remove_bucket(&self, tenant: &str, name: &str) -> RgwResult<()>;

    /// `Bucket::read_stats`. Errors: `NoSuchBucket`.
    async fn bucket_stats(&self, bucket: &BucketKey) -> RgwResult<BucketStats>;

    // ---- objects: `rgw::sal::Object` --------------------------------------

    /// `Bucket::list`. Entries are ordered bytewise by name. With a
    /// delimiter, keys sharing `prefix + <text up to and including the first
    /// delimiter after the prefix>` collapse into one common prefix that
    /// counts toward `max_keys`. `marker` compares against keys and common
    /// prefixes alike, a common prefix comparing as its own string, so a
    /// page can resume after one. Errors: `NoSuchBucket`.
    async fn list_objects(&self, bucket: &BucketKey, params: &ListParams) -> RgwResult<ListResult>;

    /// `Object::get_obj_attrs` plus the index entry. Errors: `NoSuchBucket`,
    /// `NoSuchKey`.
    async fn head_object(&self, bucket: &BucketKey, key: &ObjectKey) -> RgwResult<ObjectInfo>;

    /// `Object::ReadOp`. An `Absolute` range whose `start` is past the last
    /// byte, a `Suffix(0)`, or any range on an empty object is
    /// `InvalidRange`; an `end` past the last byte is clamped, and a
    /// `Suffix` longer than the object returns all of it. Errors:
    /// `NoSuchBucket`, `NoSuchKey`, `InvalidRange`.
    async fn get_object(
        &self,
        bucket: &BucketKey,
        key: &ObjectKey,
        range: Option<ByteRange>,
    ) -> RgwResult<ObjectRead>;

    /// `Driver::get_atomic_writer` + `Writer::complete`: stores the payload,
    /// replacing any existing object (no versioning), and returns the
    /// resulting info with `size`, `etag` (lowercase hex MD5) and `mtime`
    /// filled in and `attrs`/`owner` as given. Errors: `NoSuchBucket`.
    async fn put_object(
        &self,
        bucket: &BucketKey,
        key: &ObjectKey,
        owner: Owner,
        attrs: Attrs,
        body: ObjectBody,
    ) -> RgwResult<ObjectInfo>;

    /// `Object::DeleteOp`. Errors: `NoSuchBucket`, `NoSuchKey` (the S3
    /// layer turns the latter into a 204, as RGW does).
    async fn delete_object(&self, bucket: &BucketKey, key: &ObjectKey) -> RgwResult<()>;
}

/// Build an [`ObjectBody`] from in-memory bytes.
pub fn body_from_bytes(bytes: impl Into<Bytes>) -> ObjectBody {
    Box::pin(futures::stream::once(std::future::ready(Ok(bytes.into()))))
}

/// Drain an [`ObjectBody`] into memory.
pub async fn read_body(mut body: ObjectBody) -> io::Result<Bytes> {
    use futures::TryStreamExt;
    let mut out = bytes::BytesMut::new();
    while let Some(chunk) = body.try_next().await? {
        out.extend_from_slice(&chunk);
    }
    Ok(out.freeze())
}
