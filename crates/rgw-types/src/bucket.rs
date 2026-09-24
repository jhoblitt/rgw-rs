//! `rgw_bucket` and `RGWBucketInfo` from `src/rgw/rgw_bucket_types.h` and
//! `src/rgw/rgw_common.h`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::user::UserId;

/// `rgw_bucket`: the name plus the identity RGW assigns at creation.
///
/// `bucket_id` is the bucket instance id (`<zone id>.<n>.<m>` in RGW, any
/// unique string here) and `marker` is the prefix RGW uses for the bucket's
/// index objects. Both are kept so the admin API can report them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketKey {
    pub tenant: String,
    pub name: String,
    pub bucket_id: String,
    pub marker: String,
}

impl BucketKey {
    pub fn new(tenant: impl Into<String>, name: impl Into<String>, bucket_id: impl Into<String>) -> Self {
        let bucket_id = bucket_id.into();
        Self { tenant: tenant.into(), name: name.into(), marker: bucket_id.clone(), bucket_id }
    }

    /// `rgw_bucket::get_key`: `tenant/name`, or `name` without a tenant.
    pub fn full_name(&self) -> String {
        if self.tenant.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.tenant, self.name)
        }
    }
}

/// `RGWBucketInfo`, trimmed: no quota, versioning, website, lifecycle,
/// object lock, sync policy, or index layout beyond the shard count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketInfo {
    pub bucket: BucketKey,
    pub owner: UserId,
    pub creation_time: DateTime<Utc>,
    /// `rgw_placement_rule` in its string form `<name>/<storage class>`.
    #[serde(default)]
    pub placement_rule: String,
    #[serde(default)]
    pub zonegroup: String,
    #[serde(default)]
    pub num_shards: u32,
}

/// The counters `rgw::sal::Bucket::read_stats()` fills (`RGWStorageStats`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BucketStats {
    pub num_objects: u64,
    pub size: u64,
}
