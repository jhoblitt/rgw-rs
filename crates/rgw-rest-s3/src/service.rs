//! Service-level ops (`GET /`): `RGWListBuckets` in `rgw_op.cc` and
//! `RGWListBuckets_ObjStore_S3` in `rgw_rest_s3.cc`.

use axum::response::Response;
use rgw_auth::Identity;
use rgw_rest::iso8601_millis;
use rgw_sal::Driver;
use rgw_types::{BucketInfo, RgwResult, UserId};

use crate::error::OpResult;
use crate::perm::require_user;
use crate::xml::{BucketXml, BucketsXml, ListAllMyBucketsResult, OwnerXml, XMLNS, xml_response};

/// Page through `Driver::list_buckets` for one owner, stopping once `limit`
/// buckets are in hand. RGW pages the same way in `RGWListBuckets::execute`
/// (by `rgw_list_buckets_max_chunk`) and `check_owner_max_buckets`.
pub(crate) async fn list_owned_buckets(
    driver: &dyn Driver,
    owner: &UserId,
    limit: Option<usize>,
) -> RgwResult<Vec<BucketInfo>> {
    let mut out = Vec::new();
    let mut marker = String::new();
    loop {
        let page = driver.list_buckets(Some(owner), &marker, 0).await?;
        let last = page.buckets.last().map(|b| b.bucket.name.clone());
        out.extend(page.buckets);
        if limit.is_some_and(|l| out.len() >= l) {
            break;
        }
        match last {
            Some(last) if page.is_truncated => {
                marker = if page.next_marker.is_empty() { last } else { page.next_marker };
            }
            _ => break,
        }
    }
    Ok(out)
}

/// `RGWListBuckets`: the buckets the requester owns. Anonymous requests are
/// `AccessDenied`, as `RGWListBuckets::verify_permission` has it.
pub(crate) async fn list_buckets(driver: &dyn Driver, identity: &Identity) -> OpResult<Response> {
    let user = require_user(identity)?;
    let buckets = list_owned_buckets(driver, &user.user_id, None).await?;
    let doc = ListAllMyBucketsResult {
        xmlns: XMLNS,
        owner: OwnerXml { id: user.user_id.to_string(), display_name: user.display_name.clone() },
        buckets: BucketsXml {
            bucket: buckets
                .into_iter()
                .map(|b| BucketXml { name: b.bucket.name, creation_date: iso8601_millis(b.creation_time) })
                .collect(),
        },
    };
    Ok(xml_response(&doc)?)
}
