//! `rgw_user`, `RGWUserInfo`, `RGWAccessKey` and `RGWUserCaps` from
//! `src/rgw/rgw_user_types.h` and `src/rgw/rgw_common.h`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::RgwError;

/// `RGW_DEFAULT_MAX_BUCKETS` in `rgw_common.h`.
pub const RGW_DEFAULT_MAX_BUCKETS: i32 = 1000;

/// `rgw_user`: a user id qualified by tenant.
///
/// The string form is `tenant$id`, or a bare `id` when the tenant is empty,
/// matching `rgw_user::to_str()`. The C++ struct's `ns` field (used for
/// OIDC and role principals) is omitted.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserId {
    pub tenant: String,
    pub id: String,
}

impl UserId {
    pub fn new(id: impl Into<String>) -> Self {
        Self { tenant: String::new(), id: id.into() }
    }

    pub fn with_tenant(tenant: impl Into<String>, id: impl Into<String>) -> Self {
        Self { tenant: tenant.into(), id: id.into() }
    }

    /// `rgw_user::from_str`: split on the first `$`.
    pub fn parse(s: &str) -> Self {
        match s.split_once('$') {
            Some((tenant, id)) => Self::with_tenant(tenant, id),
            None => Self::new(s),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.tenant.is_empty() {
            f.write_str(&self.id)
        } else {
            write!(f, "{}${}", self.tenant, self.id)
        }
    }
}

impl FromStr for UserId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

impl Serialize for UserId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for UserId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(d)?))
    }
}

/// `RGWAccessKey`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessKey {
    pub id: String,
    pub key: String,
    #[serde(default)]
    pub subuser: String,
    #[serde(default = "default_true")]
    pub active: bool,
    pub create_date: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

/// `RGW_CAP_READ` / `RGW_CAP_WRITE` bit set from `rgw_common.h`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CapPerm(u8);

impl CapPerm {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(0x1);
    pub const WRITE: Self = Self(0x2);
    pub const ALL: Self = Self(0x3);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn is_none(self) -> bool {
        self.0 == 0
    }

    /// `RGWUserCaps::parse_cap_perm`: `*` or `read, write` grant both bits.
    pub fn parse(s: &str) -> Result<Self, RgwError> {
        let mut perm = Self::NONE;
        for part in s.split(',').map(str::trim) {
            perm = perm.union(match part {
                "*" => Self::ALL,
                "read" => Self::READ,
                "write" => Self::WRITE,
                other => {
                    return Err(RgwError::InvalidArgument(format!("invalid cap perm: {other}")));
                }
            });
        }
        Ok(perm)
    }
}

impl fmt::Display for CapPerm {
    /// `RGWUserCaps::perm_to_str`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            Self::ALL => "*",
            Self::READ => "read",
            Self::WRITE => "write",
            _ => "<invalid>",
        })
    }
}

/// `RGWUserCaps::is_valid_cap_type`.
const VALID_CAP_TYPES: &[&str] = &[
    "user",
    "users",
    "bucket",
    "buckets",
    "metadata",
    "info",
    "usage",
    "zone",
    "bilog",
    "mdlog",
    "datalog",
    "roles",
    "user-policy",
    "amz-cache",
    "oidc-provider",
    "ratelimit",
    "user-info-without-keys",
    "accounts",
];

/// `RGWUserCaps`: capability type (`users`, `buckets`, `usage`, ...) to
/// permission bits. Only the admin API consults these.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserCaps(BTreeMap<String, CapPerm>);

impl UserCaps {
    pub fn new() -> Self {
        Self::default()
    }

    /// `RGWUserCaps::add_cap`: bits accumulate across calls.
    pub fn add(&mut self, cap: &str, perm: CapPerm) -> Result<(), RgwError> {
        if !VALID_CAP_TYPES.contains(&cap) {
            return Err(RgwError::InvalidArgument(format!("invalid cap type: {cap}")));
        }
        let entry = self.0.entry(cap.to_owned()).or_default();
        *entry = entry.union(perm);
        Ok(())
    }

    /// `RGWUserCaps::remove_cap`: clears the bits, dropping the entry when
    /// none remain.
    pub fn remove(&mut self, cap: &str, perm: CapPerm) {
        if let Some(entry) = self.0.get_mut(cap) {
            *entry = entry.difference(perm);
            if entry.is_none() {
                self.0.remove(cap);
            }
        }
    }

    /// `RGWUserCaps::add_from_string`: `users=*;buckets=read`.
    pub fn add_from_string(&mut self, s: &str) -> Result<(), RgwError> {
        for item in s.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (cap, perm) = item
                .split_once('=')
                .ok_or_else(|| RgwError::InvalidArgument(format!("invalid cap: {item}")))?;
            self.add(cap.trim(), CapPerm::parse(perm)?)?;
        }
        Ok(())
    }

    /// `RGWUserCaps::check_cap`: the user holds every bit in `perm` for `cap`.
    pub fn check_cap(&self, cap: &str, perm: CapPerm) -> bool {
        self.0.get(cap).is_some_and(|have| have.contains(perm))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, CapPerm)> {
        self.0.iter().map(|(k, v)| (k.as_str(), *v))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One entry of the `"caps"` list as `RGWUserCaps::dump` emits it.
#[derive(Serialize, Deserialize)]
struct CapEntry {
    #[serde(rename = "type")]
    cap_type: String,
    perm: String,
}

impl Serialize for UserCaps {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let entries: Vec<CapEntry> = self
            .iter()
            .map(|(cap_type, perm)| CapEntry { cap_type: cap_type.to_owned(), perm: perm.to_string() })
            .collect();
        entries.serialize(s)
    }
}

impl<'de> Deserialize<'de> for UserCaps {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut caps = Self::new();
        for entry in Vec::<CapEntry>::deserialize(d)? {
            let perm = CapPerm::parse(&entry.perm).map_err(serde::de::Error::custom)?;
            caps.add(&entry.cap_type, perm).map_err(serde::de::Error::custom)?;
        }
        Ok(caps)
    }
}

/// `RGWUserInfo`, trimmed: no subusers, Swift keys, quotas, MFA, placement
/// or account membership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: UserId,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub user_email: String,
    /// Keyed by access key id, as the C++ `std::map<std::string, RGWAccessKey>`.
    #[serde(default)]
    pub access_keys: BTreeMap<String, AccessKey>,
    #[serde(default)]
    pub caps: UserCaps,
    #[serde(default)]
    pub suspended: bool,
    #[serde(default = "default_max_buckets")]
    pub max_buckets: i32,
    /// The `system` flag: bypasses permission checks for multisite sync.
    #[serde(default)]
    pub system: bool,
    /// The `admin` flag: every admin-API capability check passes.
    #[serde(default)]
    pub admin: bool,
    pub create_date: DateTime<Utc>,
}

fn default_max_buckets() -> i32 {
    RGW_DEFAULT_MAX_BUCKETS
}

impl UserInfo {
    pub fn new(user_id: UserId, display_name: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            user_id,
            display_name: display_name.into(),
            user_email: String::new(),
            access_keys: BTreeMap::new(),
            caps: UserCaps::new(),
            suspended: false,
            max_buckets: RGW_DEFAULT_MAX_BUCKETS,
            system: false,
            admin: false,
            create_date: now,
        }
    }

    pub fn add_access_key(&mut self, key: AccessKey) {
        self.access_keys.insert(key.id.clone(), key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_round_trips_tenant_form() {
        let id = UserId::parse("acme$alice");
        assert_eq!(id, UserId::with_tenant("acme", "alice"));
        assert_eq!(id.to_string(), "acme$alice");
        assert_eq!(UserId::parse("bob").to_string(), "bob");
    }

    #[test]
    fn caps_parse_and_check() {
        let mut caps = UserCaps::new();
        caps.add_from_string("users=*;buckets=read").unwrap();
        assert!(caps.check_cap("users", CapPerm::ALL));
        assert!(caps.check_cap("buckets", CapPerm::READ));
        assert!(!caps.check_cap("buckets", CapPerm::WRITE));
        assert!(!caps.check_cap("usage", CapPerm::READ));
        assert!(caps.add_from_string("bogus=*").is_err());
        caps.remove("buckets", CapPerm::READ);
        assert!(!caps.check_cap("buckets", CapPerm::READ));
    }

    #[test]
    fn caps_serialize_as_rgw_list() {
        let mut caps = UserCaps::new();
        caps.add_from_string("users=read, write").unwrap();
        let json = serde_json::to_string(&caps).unwrap();
        assert_eq!(json, r#"[{"type":"users","perm":"*"}]"#);
        let back: UserCaps = serde_json::from_str(&json).unwrap();
        assert_eq!(back, caps);
    }
}
