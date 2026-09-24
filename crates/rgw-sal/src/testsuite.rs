//! A conformance suite every driver runs: the executable form of the error
//! contracts on [`Driver`]. Panics on the first violation, so a backend's
//! test is one line: `rgw_sal::testsuite::run_all(&driver).await`.
//!
//! Each section uses its own bucket and user names, so the suite can run
//! against one shared, initially empty driver.

use bytes::Bytes;
use chrono::{Timelike, Utc};
use rgw_types::{
    AccessKey, Attrs, BucketInfo, BucketKey, BucketStats, ObjectKey, Owner, RGW_ATTR_CONTENT_TYPE,
    RGW_ATTR_META_PREFIX, RgwError, UserId, UserInfo,
};

use crate::{ByteRange, Driver, ListParams, ObjectBody, body_from_bytes, read_body};

pub async fn run_all(driver: &dyn Driver) {
    users(driver).await;
    buckets(driver).await;
    objects(driver).await;
    listing(driver).await;
    ranges(driver).await;
}

fn now() -> chrono::DateTime<Utc> {
    Utc::now().with_nanosecond(0).expect("zero nanoseconds is valid")
}

fn user(id: &str) -> UserInfo {
    UserInfo::new(UserId::new(id), format!("{id} display"), now())
}

fn key(id: &str) -> AccessKey {
    AccessKey {
        id: id.to_owned(),
        key: format!("secret-{id}"),
        subuser: String::new(),
        active: true,
        create_date: now(),
    }
}

fn owner(id: &str) -> Owner {
    Owner { id: UserId::new(id), display_name: format!("{id} display") }
}

fn bucket(name: &str, owner: &str) -> BucketInfo {
    BucketInfo {
        bucket: BucketKey::new("", name, format!("id-{name}")),
        owner: UserId::new(owner),
        creation_time: now(),
        placement_rule: "default-placement".to_owned(),
        zonegroup: "default".to_owned(),
        num_shards: 0,
    }
}

fn names(buckets: &[BucketInfo]) -> Vec<&str> {
    buckets.iter().map(|b| b.bucket.name.as_str()).collect()
}

async fn put(d: &dyn Driver, b: &BucketInfo, name: &str, data: &'static str) {
    d.put_object(&b.bucket, &ObjectKey::new(name), owner(&b.owner.id), Attrs::new(), body_from_bytes(data))
        .await
        .unwrap_or_else(|e| panic!("put {name}: {e}"));
}

pub async fn users(d: &dyn Driver) {
    assert!(matches!(d.load_user(&UserId::new("nobody")).await, Err(RgwError::NoSuchUser)));

    let mut alice = user("alice");
    alice.user_email = "alice@example.com".to_owned();
    alice.caps.add_from_string("users=*").unwrap();
    alice.add_access_key(key("AKALICE"));
    d.store_user(&alice, true).await.unwrap();
    assert!(matches!(d.store_user(&alice, true).await, Err(RgwError::UserAlreadyExists)));
    assert_eq!(d.load_user(&alice.user_id).await.unwrap(), alice);
    assert_eq!(d.load_user_by_access_key("AKALICE").await.unwrap(), alice);
    assert_eq!(d.load_user_by_email("alice@example.com").await.unwrap(), alice);
    assert!(matches!(d.load_user_by_access_key("AKNOPE").await, Err(RgwError::NoSuchUser)));
    assert!(matches!(d.load_user_by_email("nope@example.com").await, Err(RgwError::NoSuchUser)));

    // An access key id is unique across users.
    let mut bob = user("bob");
    bob.add_access_key(key("AKALICE"));
    assert!(matches!(d.store_user(&bob, true).await, Err(RgwError::KeyExists)));
    bob.access_keys.clear();
    bob.add_access_key(key("AKBOB"));
    d.store_user(&bob, true).await.unwrap();

    // A non-exclusive store replaces the record and its key index.
    alice.access_keys.clear();
    alice.add_access_key(key("AKALICE2"));
    alice.display_name = "Alice Two".to_owned();
    d.store_user(&alice, false).await.unwrap();
    assert!(matches!(d.load_user_by_access_key("AKALICE").await, Err(RgwError::NoSuchUser)));
    assert_eq!(d.load_user_by_access_key("AKALICE2").await.unwrap().display_name, "Alice Two");

    let all = d.list_users("", 0).await.unwrap();
    assert!(all.contains(&alice.user_id) && all.contains(&bob.user_id), "{all:?}");
    let after = d.list_users("alice", 0).await.unwrap();
    assert!(!after.contains(&alice.user_id) && after.contains(&bob.user_id), "{after:?}");
    assert_eq!(d.list_users("", 1).await.unwrap().len(), 1);

    d.remove_user(&bob.user_id).await.unwrap();
    assert!(matches!(d.load_user(&bob.user_id).await, Err(RgwError::NoSuchUser)));
    assert!(matches!(d.load_user_by_access_key("AKBOB").await, Err(RgwError::NoSuchUser)));
    assert!(matches!(d.remove_user(&bob.user_id).await, Err(RgwError::NoSuchUser)));
}

pub async fn buckets(d: &dyn Driver) {
    assert!(matches!(d.load_bucket("", "nope").await, Err(RgwError::NoSuchBucket)));

    let alice = UserId::new("alice");
    let b1 = bucket("b-alpha", "alice");
    let b2 = bucket("b-beta", "alice");
    let b3 = bucket("b-gamma", "carol");
    d.create_bucket(&b1).await.unwrap();
    assert!(matches!(d.create_bucket(&b1).await, Err(RgwError::BucketAlreadyExists)));
    d.create_bucket(&b2).await.unwrap();
    d.create_bucket(&b3).await.unwrap();
    assert_eq!(d.load_bucket("", "b-alpha").await.unwrap(), b1);

    let mine = d.list_buckets(Some(&alice), "", 0).await.unwrap();
    assert_eq!(names(&mine.buckets), ["b-alpha", "b-beta"]);
    assert!(!mine.is_truncated);
    let all = d.list_buckets(None, "", 0).await.unwrap();
    assert!(names(&all.buckets).contains(&"b-gamma"));

    let page = d.list_buckets(Some(&alice), "", 1).await.unwrap();
    assert_eq!(names(&page.buckets), ["b-alpha"]);
    assert!(page.is_truncated);
    assert_eq!(page.next_marker, "b-alpha");
    let page2 = d.list_buckets(Some(&alice), &page.next_marker, 1).await.unwrap();
    assert_eq!(names(&page2.buckets), ["b-beta"]);
    assert!(!page2.is_truncated);

    assert_eq!(d.bucket_stats(&b1.bucket).await.unwrap(), BucketStats::default());
    put(d, &b3, "x", "hi").await;
    assert!(matches!(d.remove_bucket("", "b-gamma").await, Err(RgwError::BucketNotEmpty)));
    assert_eq!(d.bucket_stats(&b3.bucket).await.unwrap(), BucketStats { num_objects: 1, size: 2 });
    d.delete_object(&b3.bucket, &ObjectKey::new("x")).await.unwrap();
    d.remove_bucket("", "b-gamma").await.unwrap();
    assert!(matches!(d.remove_bucket("", "b-gamma").await, Err(RgwError::NoSuchBucket)));
    assert!(matches!(d.load_bucket("", "b-gamma").await, Err(RgwError::NoSuchBucket)));
    assert!(matches!(
        d.list_objects(&b3.bucket, &ListParams::default()).await,
        Err(RgwError::NoSuchBucket)
    ));
}

pub async fn objects(d: &dyn Driver) {
    let b = bucket("b-objects", "alice");
    d.create_bucket(&b).await.unwrap();
    let k = ObjectKey::new("dir/hello.txt");
    assert!(matches!(d.head_object(&b.bucket, &k).await, Err(RgwError::NoSuchKey)));
    assert!(matches!(d.get_object(&b.bucket, &k, None).await, Err(RgwError::NoSuchKey)));
    assert!(matches!(d.delete_object(&b.bucket, &k).await, Err(RgwError::NoSuchKey)));

    let mut attrs = Attrs::new();
    attrs.insert(RGW_ATTR_CONTENT_TYPE.to_owned(), b"text/plain".to_vec());
    attrs.insert(format!("{RGW_ATTR_META_PREFIX}color"), b"blue".to_vec());
    let info = d
        .put_object(&b.bucket, &k, owner("alice"), attrs.clone(), body_from_bytes("hello world"))
        .await
        .unwrap();
    assert_eq!(info.key, k);
    assert_eq!(info.size, 11);
    assert_eq!(info.etag, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    assert_eq!(info.attrs, attrs);
    assert_eq!(info.owner, owner("alice"));
    assert_eq!(info.content_type(), Some("text/plain"));
    assert_eq!(info.user_metadata().collect::<Vec<_>>(), [("color", &b"blue"[..])]);

    assert_eq!(d.head_object(&b.bucket, &k).await.unwrap(), info);
    let read = d.get_object(&b.bucket, &k, None).await.unwrap();
    assert_eq!(read.info, info);
    assert_eq!(read.range, None);
    assert_eq!(read_body(read.body).await.unwrap(), "hello world");

    // Overwrite replaces data and attrs.
    let info2 = d
        .put_object(&b.bucket, &k, owner("alice"), Attrs::new(), body_from_bytes("bye"))
        .await
        .unwrap();
    assert_eq!(info2.size, 3);
    assert!(info2.attrs.is_empty());
    let read = d.get_object(&b.bucket, &k, None).await.unwrap();
    assert_eq!(read_body(read.body).await.unwrap(), "bye");
    assert_eq!(d.bucket_stats(&b.bucket).await.unwrap(), BucketStats { num_objects: 1, size: 3 });

    let empty = d
        .put_object(&b.bucket, &ObjectKey::new("empty"), owner("alice"), Attrs::new(), body_from_bytes(""))
        .await
        .unwrap();
    assert_eq!(empty.size, 0);
    assert_eq!(empty.etag, "d41d8cd98f00b204e9800998ecf8427e");
    let read = d.get_object(&b.bucket, &ObjectKey::new("empty"), None).await.unwrap();
    assert!(read_body(read.body).await.unwrap().is_empty());

    let chunks: ObjectBody =
        Box::pin(futures::stream::iter([Ok(Bytes::from("ab")), Ok(Bytes::from("cd"))]));
    let multi = d
        .put_object(&b.bucket, &ObjectKey::new("multi"), owner("alice"), Attrs::new(), chunks)
        .await
        .unwrap();
    assert_eq!(multi.size, 4);
    assert_eq!(multi.etag, "e2fc714c4727ee9395f324cd2e7f331f");
    let read = d.get_object(&b.bucket, &ObjectKey::new("multi"), None).await.unwrap();
    assert_eq!(read_body(read.body).await.unwrap(), "abcd");

    d.delete_object(&b.bucket, &k).await.unwrap();
    assert!(matches!(d.head_object(&b.bucket, &k).await, Err(RgwError::NoSuchKey)));
    assert_eq!(d.bucket_stats(&b.bucket).await.unwrap(), BucketStats { num_objects: 2, size: 4 });

    let ghost = BucketKey::new("", "ghost", "id-ghost");
    assert!(matches!(
        d.put_object(&ghost, &k, owner("alice"), Attrs::new(), body_from_bytes("x")).await,
        Err(RgwError::NoSuchBucket)
    ));
    assert!(matches!(d.head_object(&ghost, &k).await, Err(RgwError::NoSuchBucket)));
    assert!(matches!(d.delete_object(&ghost, &k).await, Err(RgwError::NoSuchBucket)));
}

pub async fn listing(d: &dyn Driver) {
    let b = bucket("b-listing", "alice");
    d.create_bucket(&b).await.unwrap();
    let all_keys = [
        "a.txt",
        "b.txt",
        "photos/2024/x.jpg",
        "photos/2024/y.jpg",
        "photos/2025/z.jpg",
        "photos/top.jpg",
        "zeta",
    ];
    // Insert out of order to check the driver sorts.
    for name in all_keys.iter().rev() {
        put(d, &b, name, "").await;
    }
    let keys = |r: &crate::ListResult| r.objects.iter().map(|o| o.key.name.clone()).collect::<Vec<_>>();

    let params = ListParams::default();
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(keys(&r), all_keys);
    assert!(r.common_prefixes.is_empty());
    assert!(!r.is_truncated);

    let params = ListParams { prefix: "photos/".into(), delimiter: "/".into(), ..Default::default() };
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(keys(&r), ["photos/top.jpg"]);
    assert_eq!(r.common_prefixes, ["photos/2024/", "photos/2025/"]);

    let params = ListParams { delimiter: "/".into(), ..Default::default() };
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(keys(&r), ["a.txt", "b.txt", "zeta"]);
    assert_eq!(r.common_prefixes, ["photos/"]);

    let params = ListParams { prefix: "b".into(), ..Default::default() };
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(keys(&r), ["b.txt"]);

    // Pagination without a delimiter.
    let params = ListParams { max_keys: 2, ..Default::default() };
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(keys(&r), ["a.txt", "b.txt"]);
    assert!(r.is_truncated);
    assert_eq!(r.next_marker, "b.txt");
    let params = ListParams { max_keys: 2, marker: r.next_marker, ..Default::default() };
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(keys(&r), ["photos/2024/x.jpg", "photos/2024/y.jpg"]);
    assert!(r.is_truncated);

    // Pagination across a common prefix.
    let params = ListParams { delimiter: "/".into(), max_keys: 3, ..Default::default() };
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(keys(&r), ["a.txt", "b.txt"]);
    assert_eq!(r.common_prefixes, ["photos/"]);
    assert!(r.is_truncated);
    assert_eq!(r.next_marker, "photos/");
    let params = ListParams { delimiter: "/".into(), max_keys: 3, marker: r.next_marker, ..Default::default() };
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(keys(&r), ["zeta"]);
    assert!(r.common_prefixes.is_empty());
    assert!(!r.is_truncated);

    // An exact page boundary is not truncated.
    let params = ListParams { max_keys: 7, ..Default::default() };
    let r = d.list_objects(&b.bucket, &params).await.unwrap();
    assert_eq!(r.objects.len(), 7);
    assert!(!r.is_truncated);
}

pub async fn ranges(d: &dyn Driver) {
    let b = bucket("b-ranges", "alice");
    d.create_bucket(&b).await.unwrap();
    put(d, &b, "digits", "0123456789").await;
    put(d, &b, "empty", "").await;
    let digits = ObjectKey::new("digits");

    let cases: [(ByteRange, (u64, u64), &str); 5] = [
        (ByteRange::Absolute { start: 0, end: Some(3) }, (0, 3), "0123"),
        (ByteRange::Absolute { start: 7, end: None }, (7, 9), "789"),
        (ByteRange::Absolute { start: 5, end: Some(100) }, (5, 9), "56789"),
        (ByteRange::Suffix(3), (7, 9), "789"),
        (ByteRange::Suffix(100), (0, 9), "0123456789"),
    ];
    for (range, want_range, want_body) in cases {
        let read = d.get_object(&b.bucket, &digits, Some(range)).await.unwrap();
        assert_eq!(read.range, Some(want_range), "{range:?}");
        assert_eq!(read.info.size, 10, "{range:?}: info reports the whole object");
        assert_eq!(read_body(read.body).await.unwrap(), want_body, "{range:?}");
    }

    let invalid = [
        (&digits, ByteRange::Absolute { start: 10, end: None }),
        (&digits, ByteRange::Suffix(0)),
        (&ObjectKey::new("empty"), ByteRange::Absolute { start: 0, end: None }),
        (&ObjectKey::new("empty"), ByteRange::Suffix(1)),
    ];
    for (key, range) in invalid {
        assert!(
            matches!(d.get_object(&b.bucket, key, Some(range)).await, Err(RgwError::InvalidRange)),
            "{key:?} {range:?}"
        );
    }
}
