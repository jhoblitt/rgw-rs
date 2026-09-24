//! The JSON documents RGW's formatter emits for admin results, key for key
//! and in RGW's order. Fields this spike does not model are kept with the
//! values RGW prints for a default record, so clients that read them keep
//! working.

use rgw_rest::iso8601_micros;
use rgw_types::{BucketInfo, BucketStats, UserId, UserInfo};
use serde_json::{Value, json};

/// `RGWQuotaInfo::dump` for a quota that was never set.
fn disabled_quota() -> Value {
    json!({
        "enabled": false,
        "check_on_raw": false,
        "max_size": -1,
        "max_size_kb": 0,
        "max_objects": -1,
    })
}

/// `dump_access_keys_info` (`RGWAccessKey::dump` per key): the array
/// `RGWUserAdminOp_Key::create` prints.
pub fn keys(info: &UserInfo) -> Value {
    let uid = info.user_id.to_string();
    info.access_keys
        .values()
        .map(|k| {
            let user = if k.subuser.is_empty() { uid.clone() } else { format!("{uid}:{}", k.subuser) };
            json!({
                "user": user,
                "access_key": k.id,
                "secret_key": k.key,
                "active": k.active,
                "create_date": iso8601_micros(k.create_date),
            })
        })
        .collect()
}

/// `RGWUserCaps::dump`: the array `RGWUserAdminOp_Caps::add` and `remove`
/// print.
pub fn caps(info: &UserInfo) -> Value {
    serde_json::to_value(&info.caps).unwrap_or_else(|_| Value::Array(Vec::new()))
}

/// `RGWUserInfo::dump`, as `RGWUserAdminOp_User::info`, `create` and
/// `modify` print it.
pub fn user_info(info: &UserInfo) -> Value {
    json!({
        "user_id": info.user_id.to_string(),
        "display_name": info.display_name,
        "email": info.user_email,
        "suspended": i32::from(info.suspended),
        "max_buckets": info.max_buckets,
        "subusers": [],
        "keys": keys(info),
        "swift_keys": [],
        "caps": caps(info),
        "op_mask": "read, write, delete",
        "system": info.system,
        "admin": info.admin,
        "default_placement": "",
        "default_storage_class": "",
        "placement_tags": [],
        "bucket_quota": disabled_quota(),
        "user_quota": disabled_quota(),
        "temp_url_keys": [],
        "type": "rgw",
        "mfa_ids": [],
        "account_id": "",
        "path": "/",
        "create_date": iso8601_micros(info.create_date),
        "tags": [],
        "group_ids": [],
    })
}

/// `RGWBucketAdminOp::info`'s per-bucket document (`dump_bucket_info`);
/// `usage` is `{}` unless stats are given.
pub fn bucket_info(info: &BucketInfo, stats: Option<&BucketStats>) -> Value {
    let created = iso8601_micros(info.creation_time);
    let usage = match stats {
        Some(s) => {
            let kb = s.size.div_ceil(1024);
            json!({
                "rgw.main": {
                    "size": s.size,
                    "size_actual": s.size,
                    "size_utilized": s.size,
                    "size_kb": kb,
                    "size_kb_actual": kb,
                    "size_kb_utilized": kb,
                    "num_objects": s.num_objects,
                }
            })
        }
        None => json!({}),
    };
    json!({
        "bucket": info.bucket.name,
        "num_shards": info.num_shards,
        "tenant": info.bucket.tenant,
        "zonegroup": info.zonegroup,
        "placement_rule": info.placement_rule,
        "explicit_placement": {
            "data_pool": "",
            "data_extra_pool": "",
            "index_pool": "",
        },
        "id": info.bucket.bucket_id,
        "marker": info.bucket.marker,
        "index_type": "Normal",
        "versioned": false,
        "versioning": false,
        "object_lock_enabled": false,
        "owner": info.owner.to_string(),
        "ver": "0#1",
        "master_ver": "0#0",
        "mtime": created,
        "creation_time": created,
        "max_marker": "0#",
        "usage": usage,
        "bucket_quota": disabled_quota(),
    })
}

/// `RGWUser::list`'s `result` section. RGW prints `marker` only when the
/// listing is truncated.
pub fn user_list(ids: &[UserId], truncated: bool, marker: &str) -> Value {
    let keys: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let mut doc = json!({
        "keys": keys,
        "truncated": truncated,
        "count": ids.len(),
    });
    if truncated && let Value::Object(map) = &mut doc {
        map.insert("marker".into(), Value::String(marker.to_owned()));
    }
    doc
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rgw_types::{AccessKey, BucketKey};

    use super::*;

    fn when() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap() + chrono::Duration::microseconds(123_456)
    }

    fn keys_of(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    fn sample_user() -> UserInfo {
        let mut info = UserInfo::new(UserId::with_tenant("acme", "alice"), "Alice", when());
        info.user_email = "alice@example.com".into();
        info.suspended = true;
        info.caps.add_from_string("users=*;buckets=read").unwrap();
        info.add_access_key(AccessKey {
            id: "AKIAEXAMPLE000000000".into(),
            key: "secret".into(),
            subuser: String::new(),
            active: true,
            create_date: when(),
        });
        info
    }

    #[test]
    fn user_info_has_rgw_keys_in_order() {
        let v = user_info(&sample_user());
        assert_eq!(
            keys_of(&v),
            [
                "user_id", "display_name", "email", "suspended", "max_buckets", "subusers", "keys",
                "swift_keys", "caps", "op_mask", "system", "admin", "default_placement",
                "default_storage_class", "placement_tags", "bucket_quota", "user_quota",
                "temp_url_keys", "type", "mfa_ids", "account_id", "path", "create_date", "tags",
                "group_ids",
            ]
        );
        assert_eq!(v["user_id"], "acme$alice");
        assert_eq!(v["suspended"], 1);
        assert_eq!(v["max_buckets"], 1000);
        assert_eq!(v["op_mask"], "read, write, delete");
        assert_eq!(v["system"], false);
        assert_eq!(v["create_date"], "2024-01-02T03:04:05.123456Z");
        assert_eq!(v["caps"], json!([{"type": "buckets", "perm": "read"}, {"type": "users", "perm": "*"}]));
        assert_eq!(
            keys_of(&v["user_quota"]),
            ["enabled", "check_on_raw", "max_size", "max_size_kb", "max_objects"]
        );
        assert_eq!(v["user_quota"]["max_size"], -1);
    }

    #[test]
    fn user_info_keys_entry() {
        let v = user_info(&sample_user());
        let key = &v["keys"][0];
        assert_eq!(keys_of(key), ["user", "access_key", "secret_key", "active", "create_date"]);
        assert_eq!(key["user"], "acme$alice");
        assert_eq!(key["access_key"], "AKIAEXAMPLE000000000");
        assert_eq!(key["active"], true);
        assert_eq!(key["create_date"], "2024-01-02T03:04:05.123456Z");
    }

    fn sample_bucket() -> BucketInfo {
        BucketInfo {
            bucket: BucketKey::new("", "b1", "zone.1.1"),
            owner: UserId::new("alice"),
            creation_time: when(),
            placement_rule: "default-placement/STANDARD".into(),
            zonegroup: "default".into(),
            num_shards: 11,
        }
    }

    #[test]
    fn bucket_info_has_rgw_keys_in_order() {
        let v = bucket_info(&sample_bucket(), None);
        assert_eq!(
            keys_of(&v),
            [
                "bucket", "num_shards", "tenant", "zonegroup", "placement_rule", "explicit_placement",
                "id", "marker", "index_type", "versioned", "versioning", "object_lock_enabled",
                "owner", "ver", "master_ver", "mtime", "creation_time", "max_marker", "usage",
                "bucket_quota",
            ]
        );
        assert_eq!(v["id"], "zone.1.1");
        assert_eq!(v["marker"], "zone.1.1");
        assert_eq!(v["owner"], "alice");
        assert_eq!(v["mtime"], "2024-01-02T03:04:05.123456Z");
        assert_eq!(v["creation_time"], v["mtime"]);
        assert_eq!(v["usage"], json!({}));
    }

    #[test]
    fn bucket_usage_rounds_kb_up() {
        let stats = BucketStats { num_objects: 3, size: 1025 };
        let v = bucket_info(&sample_bucket(), Some(&stats));
        let main = &v["usage"]["rgw.main"];
        assert_eq!(main["size"], 1025);
        assert_eq!(main["size_kb"], 2);
        assert_eq!(main["size_kb_utilized"], 2);
        assert_eq!(main["num_objects"], 3);
        let empty = bucket_info(&sample_bucket(), Some(&BucketStats::default()));
        assert_eq!(empty["usage"]["rgw.main"]["size_kb"], 0);
    }

    #[test]
    fn user_list_marker_only_when_truncated() {
        let ids = [UserId::new("a"), UserId::with_tenant("t", "b")];
        let v = user_list(&ids, false, "");
        assert_eq!(keys_of(&v), ["keys", "truncated", "count"]);
        assert_eq!(v["keys"], json!(["a", "t$b"]));
        assert_eq!(v["count"], 2);
        let v = user_list(&ids, true, "t$b");
        assert_eq!(keys_of(&v), ["keys", "truncated", "count", "marker"]);
        assert_eq!(v["marker"], "t$b");
    }
}
