//! `rgw_obj_key`, the object attribute map and `ACLOwner` from
//! `src/rgw/rgw_obj_types.h`, `rgw_sal.h` and `rgw_acl.h`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::user::UserId;

/// `RGW_ATTR_PREFIX`: every RGW xattr key starts with this.
pub const RGW_ATTR_PREFIX: &str = "user.rgw.";
pub const RGW_ATTR_ETAG: &str = "user.rgw.etag";
pub const RGW_ATTR_CONTENT_TYPE: &str = "user.rgw.content_type";
/// `RGW_ATTR_META_PREFIX`: user metadata (`x-amz-meta-*`) keys.
pub const RGW_ATTR_META_PREFIX: &str = "user.rgw.x-amz-meta-";

/// `rgw::sal::Attrs` (`std::map<std::string, ceph::buffer::list>`): opaque
/// bytes keyed by xattr name.
pub type Attrs = BTreeMap<String, Vec<u8>>;

/// `rgw_obj_key` without the namespace field.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectKey {
    pub name: String,
    /// The version id; empty for an unversioned object.
    #[serde(default)]
    pub instance: String,
}

impl ObjectKey {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), instance: String::new() }
    }
}

/// `ACLOwner`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub id: UserId,
    pub display_name: String,
}

/// What RGW keeps per object across the bucket index entry
/// (`rgw_bucket_dir_entry`) and the head object's xattrs, trimmed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub key: ObjectKey,
    pub size: u64,
    /// Lowercase hex MD5 of the payload, without the quotes HTTP adds.
    pub etag: String,
    pub mtime: DateTime<Utc>,
    pub owner: Owner,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default)]
    pub attrs: Attrs,
}

impl ObjectInfo {
    pub fn content_type(&self) -> Option<&str> {
        self.attrs.get(RGW_ATTR_CONTENT_TYPE).and_then(|v| std::str::from_utf8(v).ok())
    }

    /// User metadata with the `RGW_ATTR_META_PREFIX` stripped, so the keys
    /// are the `x-amz-meta-` suffixes the client sent.
    pub fn user_metadata(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.attrs
            .iter()
            .filter_map(|(k, v)| k.strip_prefix(RGW_ATTR_META_PREFIX).map(|k| (k, v.as_slice())))
    }
}
