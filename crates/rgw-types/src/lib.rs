//! Domain types shared by every crate in the workspace.
//!
//! Each type names the C++ type it mirrors under `src/rgw` in ceph/ceph,
//! trimmed to the fields this spike exercises. Nothing here knows about HTTP,
//! XML or SQL; those live in the REST and driver crates, just as
//! `rgw_common.h` sits below `rgw_rest*.h` and the drivers in C++.

pub mod bucket;
pub mod error;
pub mod object;
pub mod user;

pub use bucket::{BucketInfo, BucketKey, BucketStats};
pub use error::{RgwError, RgwResult};
pub use object::{
    Attrs, ObjectInfo, ObjectKey, Owner, RGW_ATTR_CONTENT_TYPE, RGW_ATTR_ETAG,
    RGW_ATTR_META_PREFIX, RGW_ATTR_PREFIX,
};
pub use user::{AccessKey, CapPerm, RGW_DEFAULT_MAX_BUCKETS, UserCaps, UserId, UserInfo};
