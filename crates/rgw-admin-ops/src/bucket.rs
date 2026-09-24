//! Bucket operations: `RGWBucketAdminOp` in `rgw_bucket.cc`.

use rgw_sal::{Driver, ListParams};
use rgw_types::{BucketInfo, BucketKey, BucketStats, RgwError, RgwResult, UserId};

/// `rgw_parse_url_bucket`: split `tenant/name` into its halves; a bare
/// name has an empty tenant.
pub fn parse_bucket(s: &str) -> (String, String) {
    match s.split_once('/') {
        Some((tenant, name)) => (tenant.to_owned(), name.to_owned()),
        None => (String::new(), s.to_owned()),
    }
}

fn require_name(name: &str) -> RgwResult<()> {
    if name.is_empty() {
        return Err(RgwError::InvalidArgument("bucket is required".into()));
    }
    Ok(())
}

pub(crate) async fn list_all_buckets(driver: &dyn Driver, owner: Option<&UserId>) -> RgwResult<Vec<BucketInfo>> {
    let mut out = Vec::new();
    let mut marker = String::new();
    loop {
        let page = driver.list_buckets(owner, &marker, 0).await?;
        out.extend(page.buckets);
        if !page.is_truncated {
            return Ok(out);
        }
        // A driver that reports truncation without advancing would loop forever.
        if page.next_marker.is_empty() || page.next_marker == marker {
            return Err(RgwError::internal("bucket listing did not advance"));
        }
        marker = page.next_marker;
    }
}

/// Delete every object in `bucket`, one listing page at a time.
pub(crate) async fn purge_objects(driver: &dyn Driver, bucket: &BucketKey) -> RgwResult<()> {
    let mut params = ListParams::default();
    loop {
        let page = driver.list_objects(bucket, &params).await?;
        for obj in &page.objects {
            match driver.delete_object(bucket, &obj.key).await {
                Ok(()) | Err(RgwError::NoSuchKey) => {}
                Err(e) => return Err(e),
            }
        }
        if !page.is_truncated {
            return Ok(());
        }
        if page.next_marker.is_empty() || page.next_marker == params.marker {
            return Err(RgwError::internal("object listing did not advance"));
        }
        params.marker = page.next_marker;
    }
}

/// `RGWBucketAdminOp::info` for one bucket: its record plus `read_stats`.
pub async fn bucket_info(driver: &dyn Driver, tenant: &str, name: &str) -> RgwResult<(BucketInfo, BucketStats)> {
    require_name(name)?;
    let info = driver.load_bucket(tenant, name).await?;
    let stats = driver.bucket_stats(&info.bucket).await?;
    Ok((info, stats))
}

/// `RGWBucketAdminOp::info` without a bucket name: every bucket, or
/// `owner`'s, with all listing pages concatenated.
pub async fn bucket_list(driver: &dyn Driver, owner: Option<&UserId>) -> RgwResult<Vec<BucketInfo>> {
    list_all_buckets(driver, owner).await
}

/// `RGWBucketAdminOp::remove_bucket`. Without `purge_objects` a non-empty
/// bucket is `BucketNotEmpty`, from the driver.
pub async fn bucket_remove(driver: &dyn Driver, tenant: &str, name: &str, purge: bool) -> RgwResult<()> {
    require_name(name)?;
    if purge {
        let info = driver.load_bucket(tenant, name).await?;
        purge_objects(driver, &info.bucket).await?;
    }
    driver.remove_bucket(tenant, name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bucket_splits_tenant() {
        assert_eq!(parse_bucket("acme/b1"), ("acme".into(), "b1".into()));
        assert_eq!(parse_bucket("b1"), (String::new(), "b1".into()));
    }
}
