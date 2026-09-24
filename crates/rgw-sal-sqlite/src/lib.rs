//! A SAL driver on SQLite: the counterpart of `driver/dbstore`, which keeps
//! users, buckets and objects (data included) in one SQLite file.
//!
//! STUB: every method returns `NotImplemented`. The backend worker replaces
//! this and makes `tests::conformance` pass.

use std::path::Path;

use async_trait::async_trait;
use rgw_sal::{BucketList, ByteRange, Driver, ListParams, ListResult, ObjectBody, ObjectRead};
use rgw_types::{
    Attrs, BucketInfo, BucketKey, BucketStats, ObjectInfo, ObjectKey, Owner, RgwError, RgwResult,
    UserId, UserInfo,
};

pub struct SqliteDriver {}

impl SqliteDriver {
    pub fn open(_path: &Path) -> RgwResult<Self> {
        Ok(Self {})
    }

    pub fn open_in_memory() -> RgwResult<Self> {
        Ok(Self {})
    }
}

#[async_trait]
impl Driver for SqliteDriver {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    async fn load_user(&self, _id: &UserId) -> RgwResult<UserInfo> {
        Err(RgwError::NotImplemented)
    }

    async fn load_user_by_access_key(&self, _access_key: &str) -> RgwResult<UserInfo> {
        Err(RgwError::NotImplemented)
    }

    async fn load_user_by_email(&self, _email: &str) -> RgwResult<UserInfo> {
        Err(RgwError::NotImplemented)
    }

    async fn store_user(&self, _info: &UserInfo, _exclusive: bool) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }

    async fn remove_user(&self, _id: &UserId) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }

    async fn list_users(&self, _marker: &str, _max: usize) -> RgwResult<Vec<UserId>> {
        Err(RgwError::NotImplemented)
    }

    async fn create_bucket(&self, _info: &BucketInfo) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }

    async fn load_bucket(&self, _tenant: &str, _name: &str) -> RgwResult<BucketInfo> {
        Err(RgwError::NotImplemented)
    }

    async fn list_buckets(&self, _owner: Option<&UserId>, _marker: &str, _max: usize) -> RgwResult<BucketList> {
        Err(RgwError::NotImplemented)
    }

    async fn remove_bucket(&self, _tenant: &str, _name: &str) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }

    async fn bucket_stats(&self, _bucket: &BucketKey) -> RgwResult<BucketStats> {
        Err(RgwError::NotImplemented)
    }

    async fn list_objects(&self, _bucket: &BucketKey, _params: &ListParams) -> RgwResult<ListResult> {
        Err(RgwError::NotImplemented)
    }

    async fn head_object(&self, _bucket: &BucketKey, _key: &ObjectKey) -> RgwResult<ObjectInfo> {
        Err(RgwError::NotImplemented)
    }

    async fn get_object(&self, _bucket: &BucketKey, _key: &ObjectKey, _range: Option<ByteRange>) -> RgwResult<ObjectRead> {
        Err(RgwError::NotImplemented)
    }

    async fn put_object(&self, _bucket: &BucketKey, _key: &ObjectKey, _owner: Owner, _attrs: Attrs, _body: ObjectBody) -> RgwResult<ObjectInfo> {
        Err(RgwError::NotImplemented)
    }

    async fn delete_object(&self, _bucket: &BucketKey, _key: &ObjectKey) -> RgwResult<()> {
        Err(RgwError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn conformance() {
        let driver = SqliteDriver::open_in_memory().unwrap();
        rgw_sal::testsuite::run_all(&driver).await;
    }
}
