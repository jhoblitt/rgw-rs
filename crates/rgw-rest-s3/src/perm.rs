//! Access checks: `RGWOp::verify_permission` for the ops this crate serves,
//! collapsed to bucket ownership, plus the bucket load RGW performs in
//! `RGWHandler::init_processing` / `rgw_build_bucket_policies` before any
//! op runs.
//!
//! RGW evaluates bucket policy, ACLs and IAM policy here; the spike keeps
//! only the owner grant and the `system` / `admin` bypass.

use rgw_auth::Identity;
use rgw_sal::Driver;
use rgw_types::{BucketInfo, RgwError, RgwResult, UserInfo};

/// The authenticated user, or `AccessDenied` for anonymous requests
/// (`RGWListBuckets::verify_permission`, `RGWCreateBucket::verify_permission`).
pub(crate) fn require_user(identity: &Identity) -> RgwResult<&UserInfo> {
    identity.user().ok_or(RgwError::AccessDenied)
}

/// Allow the bucket's owner and `system` or `admin` users; deny everyone
/// else, anonymous included.
pub(crate) fn verify_bucket_access(identity: &Identity, bucket: &BucketInfo) -> RgwResult<()> {
    match identity.user() {
        Some(u) if u.system || u.admin || u.user_id == bucket.owner => Ok(()),
        _ => Err(RgwError::AccessDenied),
    }
}

/// Load a bucket in the default tenant and check access to it.
///
/// A name containing `/` can only come from a percent-encoded `%2F` in the
/// bucket segment; no valid bucket carries one, so it is `NoSuchBucket`
/// without asking the driver.
pub(crate) async fn load_bucket_for(
    driver: &dyn Driver,
    identity: &Identity,
    name: &str,
) -> RgwResult<BucketInfo> {
    if name.contains('/') {
        return Err(RgwError::NoSuchBucket);
    }
    let bucket = driver.load_bucket("", name).await?;
    verify_bucket_access(identity, &bucket)?;
    Ok(bucket)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::Utc;
    use rgw_auth::AuthedUser;
    use rgw_types::{BucketKey, UserId};

    pub(crate) fn user(id: &str) -> UserInfo {
        UserInfo::new(UserId::new(id), format!("{id} name"), Utc::now())
    }

    pub(crate) fn identity(info: UserInfo) -> Identity {
        Identity::User(AuthedUser { info, access_key_id: "AK".into() })
    }

    fn bucket(owner: &str) -> BucketInfo {
        BucketInfo {
            bucket: BucketKey::new("", "b", "id"),
            owner: UserId::new(owner),
            creation_time: Utc::now(),
            placement_rule: String::new(),
            zonegroup: String::new(),
            num_shards: 0,
        }
    }

    #[test]
    fn owner_allowed_others_denied() {
        let b = bucket("alice");
        assert_eq!(verify_bucket_access(&identity(user("alice")), &b), Ok(()));
        assert_eq!(verify_bucket_access(&identity(user("bob")), &b), Err(RgwError::AccessDenied));
        assert_eq!(verify_bucket_access(&Identity::Anonymous, &b), Err(RgwError::AccessDenied));
    }

    #[test]
    fn tenant_is_part_of_ownership() {
        let b = bucket("alice");
        let mut other = user("alice");
        other.user_id = UserId::with_tenant("acme", "alice");
        assert_eq!(verify_bucket_access(&identity(other), &b), Err(RgwError::AccessDenied));
    }

    #[test]
    fn system_and_admin_bypass() {
        let b = bucket("alice");
        let mut sys = user("sync");
        sys.system = true;
        let mut adm = user("root");
        adm.admin = true;
        assert_eq!(verify_bucket_access(&identity(sys), &b), Ok(()));
        assert_eq!(verify_bucket_access(&identity(adm), &b), Ok(()));
    }

    #[test]
    fn anonymous_has_no_user() {
        assert_eq!(require_user(&Identity::Anonymous).err(), Some(RgwError::AccessDenied));
        assert!(require_user(&identity(user("alice"))).is_ok());
    }
}
