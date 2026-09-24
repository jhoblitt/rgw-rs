//! Bucket ops: `RGWCreateBucket`, `RGWDeleteBucket`, `RGWStatBucket`,
//! `RGWGetBucketLocation` and `RGWGetBucketVersioning` from `rgw_op.cc`,
//! with their `_ObjStore_S3` response halves from `rgw_rest_s3.cc`.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use rgw_auth::Identity;
use rgw_sal::Driver;
use rgw_types::{BucketInfo, BucketKey, RgwError, RgwResult};

use crate::error::{OpResult, header_value};
use crate::perm::{load_bucket_for, require_user};
use crate::service::list_owned_buckets;
use crate::xml::{LocationConstraint, VersioningConfiguration, XMLNS, xml_response};

/// `rgw_placement_rule` for every bucket this spike creates.
pub(crate) const DEFAULT_PLACEMENT: &str = "default-placement";
/// The zonegroup name, also what `?location` reports.
pub(crate) const DEFAULT_ZONEGROUP: &str = "default";

/// `valid_s3_bucket_name` in `rgw_rest_s3.h` with `relaxed=false`
/// (`rgw_relaxed_s3_bucket_names=false`).
pub(crate) fn validate_bucket_name(name: &str) -> RgwResult<()> {
    let b = name.as_bytes();
    let invalid = Err(RgwError::InvalidBucketName);
    if !(3..=63).contains(&b.len()) {
        return invalid;
    }
    let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    if !alnum(b[0]) || !alnum(b[b.len() - 1]) {
        return invalid;
    }
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'a'..=b'z' | b'0'..=b'9' | b'-' => {}
            // First and last are alphanumeric, so both neighbours exist.
            b'.' if b[i - 1] != b'-' && b[i + 1] != b'.' && b[i + 1] != b'-' => {}
            _ => return invalid,
        }
    }
    if looks_like_ipv4(name) {
        return invalid;
    }
    Ok(())
}

/// The IPv4 half of `looks_like_ip_address`: exactly four dot-separated
/// runs of digits. The IPv6 half cannot match a name that passed the
/// character check, since it has no `:`.
fn looks_like_ipv4(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}

/// `RGWCreateBucket`. Always creates in the empty tenant with the default
/// placement; a `CreateBucketConfiguration` body is not read.
pub(crate) async fn create_bucket(driver: &dyn Driver, identity: &Identity, name: &str) -> OpResult<Response> {
    let user = require_user(identity)?;
    validate_bucket_name(name)?;

    // `check_owner_max_buckets`: negative disables creation, 0 is unlimited.
    if user.max_buckets < 0 {
        return Err(RgwError::AccessDenied.into());
    }
    if user.max_buckets > 0 {
        let max = user.max_buckets as usize;
        let owned = list_owned_buckets(driver, &user.user_id, Some(max)).await?;
        if owned.len() >= max {
            return Err(RgwError::TooManyBuckets.into());
        }
    }

    let info = BucketInfo {
        bucket: BucketKey::new("", name, uuid::Uuid::new_v4().as_simple().to_string()),
        owner: user.user_id.clone(),
        creation_time: Utc::now(),
        placement_rule: DEFAULT_PLACEMENT.to_owned(),
        zonegroup: DEFAULT_ZONEGROUP.to_owned(),
        num_shards: 0,
    };
    match driver.create_bucket(&info).await {
        Ok(()) => {}
        Err(RgwError::BucketAlreadyExists) => {
            let owned_by_you = driver
                .load_bucket("", name)
                .await
                .is_ok_and(|existing| existing.owner == user.user_id);
            return Err(if owned_by_you {
                RgwError::BucketAlreadyOwnedByYou
            } else {
                RgwError::BucketAlreadyExists
            }
            .into());
        }
        Err(e) => return Err(e.into()),
    }
    let location = header_value(&format!("/{name}"))?;
    Ok((StatusCode::OK, [(header::LOCATION, location)]).into_response())
}

/// `RGWDeleteBucket`: 204 on success; the driver reports `BucketNotEmpty`.
pub(crate) async fn delete_bucket(driver: &dyn Driver, identity: &Identity, name: &str) -> OpResult<Response> {
    let bucket = load_bucket_for(driver, identity, name).await?;
    driver.remove_bucket(&bucket.bucket.tenant, &bucket.bucket.name).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `RGWStatBucket` (HEAD bucket). Carries the object count and byte usage
/// headers `RGWStatBucket_ObjStore_S3::send_response` dumps.
pub(crate) async fn stat_bucket(driver: &dyn Driver, identity: &Identity, name: &str) -> OpResult<Response> {
    let bucket = load_bucket_for(driver, identity, name).await?;
    let stats = driver.bucket_stats(&bucket.bucket).await?;
    Ok((
        StatusCode::OK,
        [
            ("x-rgw-object-count", header_value(&stats.num_objects.to_string())?),
            ("x-rgw-bytes-used", header_value(&stats.size.to_string())?),
        ],
    )
        .into_response())
}

/// `RGWGetBucketLocation`: the bucket's zonegroup.
pub(crate) async fn get_location(driver: &dyn Driver, identity: &Identity, name: &str) -> OpResult<Response> {
    let bucket = load_bucket_for(driver, identity, name).await?;
    let location = if bucket.zonegroup.is_empty() { DEFAULT_ZONEGROUP.to_owned() } else { bucket.zonegroup };
    Ok(xml_response(&LocationConstraint { xmlns: XMLNS, location })?)
}

/// `RGWGetBucketVersioning`: buckets here are never versioned, so the
/// document is always empty.
pub(crate) async fn get_versioning(driver: &dyn Driver, identity: &Identity, name: &str) -> OpResult<Response> {
    load_bucket_for(driver, identity, name).await?;
    Ok(xml_response(&VersioningConfiguration { xmlns: XMLNS })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        for ok in ["abc", "my-bucket", "my.bucket", "a1b", "123", "1.2.3", "1.2.3.4a", &"a".repeat(63)] {
            assert_eq!(validate_bucket_name(ok), Ok(()), "{ok}");
        }
    }

    #[test]
    fn invalid_names() {
        for bad in [
            "ab",
            &"a".repeat(64),
            "Abc",
            "abC",
            "a_b",
            "-ab",
            "ab-",
            ".ab",
            "ab.",
            "a..b",
            "a.-b",
            "a-.b",
            "a/b",
            "a b",
            "192.168.1.1",
            "1.2.3.4",
            "",
        ] {
            assert_eq!(validate_bucket_name(bad), Err(RgwError::InvalidBucketName), "{bad}");
        }
    }
}
