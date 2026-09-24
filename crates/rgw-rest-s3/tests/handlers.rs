//! End-to-end handler tests over an in-memory `Driver`, driven through the
//! router with `tower::ServiceExt::oneshot`. The identity and request id
//! the auth and request-id middleware would insert are injected as
//! `Extension` layers.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use chrono::Utc;
use md5::{Digest, Md5};
use rgw_auth::{AuthedUser, Identity};
use rgw_rest::RequestId;
use rgw_rest_s3::{S3State, router};
use rgw_sal::{BucketList, ByteRange, Driver, ListParams, ListResult, ObjectBody, ObjectRead, read_body};
use rgw_types::{
    Attrs, BucketInfo, BucketKey, BucketStats, ObjectInfo, ObjectKey, Owner, RgwError, RgwResult, UserId,
    UserInfo,
};
use tower::ServiceExt;

#[derive(Default)]
struct Mem {
    buckets: BTreeMap<String, BucketInfo>,
    /// Keyed by (bucket id, object name).
    objects: BTreeMap<(String, String), (ObjectInfo, Bytes)>,
}

#[derive(Default)]
struct MemDriver(Mutex<Mem>);

impl MemDriver {
    fn mem(&self) -> std::sync::MutexGuard<'_, Mem> {
        self.0.lock().unwrap()
    }
}

#[async_trait]
impl Driver for MemDriver {
    fn name(&self) -> &'static str {
        "mem"
    }
    async fn load_user(&self, _: &UserId) -> RgwResult<UserInfo> {
        Err(RgwError::NoSuchUser)
    }
    async fn load_user_by_access_key(&self, _: &str) -> RgwResult<UserInfo> {
        Err(RgwError::NoSuchUser)
    }
    async fn load_user_by_email(&self, _: &str) -> RgwResult<UserInfo> {
        Err(RgwError::NoSuchUser)
    }
    async fn store_user(&self, _: &UserInfo, _: bool) -> RgwResult<()> {
        Ok(())
    }
    async fn remove_user(&self, _: &UserId) -> RgwResult<()> {
        Err(RgwError::NoSuchUser)
    }
    async fn list_users(&self, _: &str, _: usize) -> RgwResult<Vec<UserId>> {
        Ok(vec![])
    }
    async fn create_bucket(&self, info: &BucketInfo) -> RgwResult<()> {
        let mut m = self.mem();
        if m.buckets.contains_key(&info.bucket.name) {
            return Err(RgwError::BucketAlreadyExists);
        }
        m.buckets.insert(info.bucket.name.clone(), info.clone());
        Ok(())
    }
    async fn load_bucket(&self, _: &str, name: &str) -> RgwResult<BucketInfo> {
        self.mem().buckets.get(name).cloned().ok_or(RgwError::NoSuchBucket)
    }
    async fn list_buckets(&self, owner: Option<&UserId>, marker: &str, max: usize) -> RgwResult<BucketList> {
        let max = if max == 0 { 1000 } else { max };
        let m = self.mem();
        let all: Vec<BucketInfo> = m
            .buckets
            .values()
            .filter(|b| owner.is_none_or(|o| &b.owner == o) && b.bucket.name.as_str() > marker)
            .cloned()
            .collect();
        let is_truncated = all.len() > max;
        let buckets: Vec<BucketInfo> = all.into_iter().take(max).collect();
        let next_marker = buckets.last().map(|b| b.bucket.name.clone()).unwrap_or_default();
        Ok(BucketList { buckets, is_truncated, next_marker })
    }
    async fn remove_bucket(&self, _: &str, name: &str) -> RgwResult<()> {
        let mut m = self.mem();
        let id = m.buckets.get(name).ok_or(RgwError::NoSuchBucket)?.bucket.bucket_id.clone();
        if m.objects.keys().any(|(b, _)| *b == id) {
            return Err(RgwError::BucketNotEmpty);
        }
        m.buckets.remove(name);
        Ok(())
    }
    async fn bucket_stats(&self, bucket: &BucketKey) -> RgwResult<BucketStats> {
        let m = self.mem();
        let mut stats = BucketStats::default();
        for ((b, _), (info, _)) in &m.objects {
            if *b == bucket.bucket_id {
                stats.num_objects += 1;
                stats.size += info.size;
            }
        }
        Ok(stats)
    }
    async fn list_objects(&self, bucket: &BucketKey, p: &ListParams) -> RgwResult<ListResult> {
        let max = if p.max_keys == 0 { 1000 } else { p.max_keys };
        let m = self.mem();
        let mut res = ListResult::default();
        let mut count = 0;
        for ((b, name), (info, _)) in &m.objects {
            if *b != bucket.bucket_id || !name.starts_with(&p.prefix) {
                continue;
            }
            let entry = match (!p.delimiter.is_empty())
                .then(|| name[p.prefix.len()..].find(&p.delimiter))
                .flatten()
            {
                Some(i) => name[..p.prefix.len() + i + p.delimiter.len()].to_owned(),
                None => name.clone(),
            };
            if entry.as_str() <= p.marker.as_str() || res.common_prefixes.last() == Some(&entry) {
                continue;
            }
            if count == max {
                res.is_truncated = true;
                break;
            }
            count += 1;
            res.next_marker = entry.clone();
            if entry == *name {
                res.objects.push(info.clone());
            } else {
                res.common_prefixes.push(entry);
            }
        }
        Ok(res)
    }
    async fn head_object(&self, bucket: &BucketKey, key: &ObjectKey) -> RgwResult<ObjectInfo> {
        let m = self.mem();
        m.objects
            .get(&(bucket.bucket_id.clone(), key.name.clone()))
            .map(|(i, _)| i.clone())
            .ok_or(RgwError::NoSuchKey)
    }
    async fn get_object(&self, bucket: &BucketKey, key: &ObjectKey, range: Option<ByteRange>) -> RgwResult<ObjectRead> {
        let (info, data) = {
            let m = self.mem();
            m.objects.get(&(bucket.bucket_id.clone(), key.name.clone())).cloned().ok_or(RgwError::NoSuchKey)?
        };
        let size = data.len() as u64;
        let resolved = match range {
            None => None,
            Some(r) => {
                if size == 0 {
                    return Err(RgwError::InvalidRange);
                }
                Some(match r {
                    ByteRange::Absolute { start, end } if start < size => {
                        (start, end.map_or(size - 1, |e| e.min(size - 1)))
                    }
                    ByteRange::Suffix(n) if n > 0 => (size.saturating_sub(n), size - 1),
                    _ => return Err(RgwError::InvalidRange),
                })
            }
        };
        let body = match resolved {
            Some((s, e)) => data.slice(s as usize..=e as usize),
            None => data,
        };
        Ok(ObjectRead { info, range: resolved, body: rgw_sal::body_from_bytes(body) })
    }
    async fn put_object(
        &self,
        bucket: &BucketKey,
        key: &ObjectKey,
        owner: Owner,
        attrs: Attrs,
        body: ObjectBody,
    ) -> RgwResult<ObjectInfo> {
        let data = read_body(body).await.map_err(RgwError::internal)?;
        let info = ObjectInfo {
            key: key.clone(),
            size: data.len() as u64,
            etag: hex::encode(Md5::digest(&data)),
            mtime: Utc::now(),
            owner,
            storage_class: String::new(),
            attrs,
        };
        self.mem().objects.insert((bucket.bucket_id.clone(), key.name.clone()), (info.clone(), data));
        Ok(info)
    }
    async fn delete_object(&self, bucket: &BucketKey, key: &ObjectKey) -> RgwResult<()> {
        self.mem()
            .objects
            .remove(&(bucket.bucket_id.clone(), key.name.clone()))
            .map(|_| ())
            .ok_or(RgwError::NoSuchKey)
    }
}

fn user(id: &str) -> Identity {
    let info = UserInfo::new(UserId::new(id), format!("{id} name"), Utc::now());
    Identity::User(AuthedUser { info, access_key_id: format!("AK{id}") })
}

struct Harness {
    driver: Arc<MemDriver>,
}

impl Harness {
    fn new() -> Self {
        Self { driver: Arc::new(MemDriver::default()) }
    }

    fn app(&self, identity: Identity) -> Router {
        router(S3State { driver: self.driver.clone() })
            .layer(Extension(identity))
            .layer(Extension(RequestId("tx-test".into())))
    }

    async fn send(&self, identity: &Identity, req: Request<Body>) -> Response {
        self.app(identity.clone()).oneshot(req).await.unwrap()
    }

    async fn call(&self, identity: &Identity, method: &str, uri: &str, body: &'static [u8]) -> Response {
        let req = Request::builder().method(method).uri(uri).body(Body::from(body)).unwrap();
        self.send(identity, req).await
    }
}

async fn text(resp: Response) -> String {
    String::from_utf8(to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap()
}

fn header<'a>(resp: &'a Response, name: &str) -> &'a str {
    resp.headers().get(name).map(|v| v.to_str().unwrap()).unwrap_or("")
}

#[tokio::test]
async fn bucket_lifecycle() {
    let h = Harness::new();
    let alice = user("alice");

    let resp = h.call(&alice, "PUT", "/photos", b"").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header(&resp, "location"), "/photos");

    let resp = h.call(&alice, "PUT", "/photos", b"").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(text(resp).await.contains("<Code>BucketAlreadyOwnedByYou</Code>"));

    let resp = h.call(&user("bob"), "PUT", "/photos", b"").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(text(resp).await.contains("<Code>BucketAlreadyExists</Code>"));

    let resp = h.call(&alice, "PUT", "/Bad_Name", b"").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = text(resp).await;
    assert!(body.contains("<Code>InvalidBucketName</Code>"), "{body}");
    assert!(body.contains("<Resource>/Bad_Name</Resource>"), "{body}");
    assert!(body.contains("<RequestId>tx-test</RequestId>"), "{body}");

    let resp = h.call(&alice, "GET", "/", b"").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = text(resp).await;
    assert!(body.contains("<Owner><ID>alice</ID><DisplayName>alice name</DisplayName></Owner>"), "{body}");
    assert!(body.contains("<Bucket><Name>photos</Name><CreationDate>"), "{body}");

    let resp = h.call(&Identity::Anonymous, "GET", "/", b"").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = h.call(&alice, "HEAD", "/photos", b"").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header(&resp, "x-rgw-object-count"), "0");
    let resp = h.call(&alice, "HEAD", "/nope", b"").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(text(resp).await.is_empty());
    let resp = h.call(&user("bob"), "HEAD", "/photos", b"").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = h.call(&alice, "GET", "/photos?location", b"").await;
    assert!(text(resp).await.ends_with(
        "<LocationConstraint xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">default</LocationConstraint>"
    ));
    let resp = h.call(&alice, "GET", "/photos?versioning", b"").await;
    assert!(text(resp).await.ends_with("<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"/>"));
    let resp = h.call(&alice, "GET", "/photos?acl", b"").await;
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    assert!(text(resp).await.contains("<Code>NotImplemented</Code>"));

    h.call(&alice, "PUT", "/photos/a", b"x").await;
    let resp = h.call(&alice, "DELETE", "/photos", b"").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    h.call(&alice, "DELETE", "/photos/a", b"").await;
    let resp = h.call(&alice, "DELETE", "/photos", b"").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = h.call(&alice, "DELETE", "/photos", b"").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn max_buckets_enforced() {
    let h = Harness::new();
    let Identity::User(mut u) = user("carol") else { unreachable!() };
    u.info.max_buckets = 2;
    let carol = Identity::User(u);
    assert_eq!(h.call(&carol, "PUT", "/bk1", b"").await.status(), StatusCode::OK);
    assert_eq!(h.call(&carol, "PUT", "/bk2", b"").await.status(), StatusCode::OK);
    let resp = h.call(&carol, "PUT", "/bk3", b"").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(text(resp).await.contains("<Code>TooManyBuckets</Code>"));
}

#[tokio::test]
async fn object_round_trip() {
    let h = Harness::new();
    let alice = user("alice");
    h.call(&alice, "PUT", "/bk1", b"").await;

    let req = Request::builder()
        .method("PUT")
        .uri("/bk1/dir/sub/hello%20world.txt")
        .header("content-type", "text/plain")
        .header("x-amz-meta-color", "blue")
        .header("x-amz-storage-class", "STANDARD_IA")
        .header("content-md5", "XUFAKrxLKna5cZ2REBfFkg==")
        .body(Body::from("hello"))
        .unwrap();
    let resp = h.send(&alice, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header(&resp, "etag"), "\"5d41402abc4b2a76b9719d911017c592\"");

    let resp = h.call(&alice, "GET", "/bk1/dir/sub/hello%20world.txt", b"").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header(&resp, "content-type"), "text/plain");
    assert_eq!(header(&resp, "content-length"), "5");
    assert_eq!(header(&resp, "x-amz-meta-color"), "blue");
    assert_eq!(header(&resp, "x-amz-storage-class"), "STANDARD_IA");
    assert_eq!(header(&resp, "accept-ranges"), "bytes");
    assert!(header(&resp, "last-modified").ends_with(" GMT"));
    assert_eq!(text(resp).await, "hello");

    let req = Request::builder()
        .uri("/bk1/dir/sub/hello%20world.txt")
        .header("range", "bytes=1-3")
        .body(Body::empty())
        .unwrap();
    let resp = h.send(&alice, req).await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(header(&resp, "content-range"), "bytes 1-3/5");
    assert_eq!(text(resp).await, "ell");

    let req = Request::builder()
        .uri("/bk1/dir/sub/hello%20world.txt")
        .header("range", "bytes=9-")
        .body(Body::empty())
        .unwrap();
    let resp = h.send(&alice, req).await;
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(header(&resp, "content-range"), "bytes */5");
    assert!(text(resp).await.contains("<Code>InvalidRange</Code>"));

    let req = Request::builder()
        .method("HEAD")
        .uri("/bk1/dir/sub/hello%20world.txt")
        .header("range", "bytes=-2")
        .body(Body::empty())
        .unwrap();
    let resp = h.send(&alice, req).await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(header(&resp, "content-range"), "bytes 3-4/5");

    let resp = h.call(&alice, "HEAD", "/bk1/missing", b"").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(text(resp).await.is_empty());

    let resp = h.call(&user("bob"), "GET", "/bk1/dir/sub/hello%20world.txt", b"").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let resp = h.call(&Identity::Anonymous, "PUT", "/bk1/x", b"").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = h.call(&alice, "DELETE", "/bk1/dir/sub/hello%20world.txt", b"").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = h.call(&alice, "DELETE", "/bk1/dir/sub/hello%20world.txt", b"").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = h.call(&alice, "GET", "/bk1/dir/sub/hello%20world.txt", b"").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(text(resp).await.contains("<Code>NoSuchKey</Code>"));
}

#[tokio::test]
async fn put_rejections() {
    let h = Harness::new();
    let alice = user("alice");
    h.call(&alice, "PUT", "/bk1", b"").await;

    let put = |md5: &'static str| {
        Request::builder().method("PUT").uri("/bk1/k").header("content-md5", md5).body(Body::from("hello")).unwrap()
    };
    let resp = h.send(&alice, put("AAAAAAAAAAAAAAAAAAAAAA==")).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(text(resp).await.contains("<Code>BadDigest</Code>"));
    let resp = h.send(&alice, put("not-base64!")).await;
    assert!(text(resp).await.contains("<Code>InvalidDigest</Code>"));
    assert_eq!(h.call(&alice, "HEAD", "/bk1/k", b"").await.status(), StatusCode::NOT_FOUND);

    let req = Request::builder()
        .method("PUT")
        .uri("/bk1/k")
        .header("x-amz-copy-source", "/bk1/other")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(&alice, req).await.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(h.call(&alice, "GET", "/bk1/k?tagging", b"").await.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(h.call(&alice, "POST", "/bk1/k?uploads", b"").await.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(h.call(&alice, "PUT", "/nobucket/k", b"x").await.status(), StatusCode::NOT_FOUND);

    let resp = h.call(&alice, "PUT", "/bk1/plain", b"data").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = h.call(&alice, "GET", "/bk1/plain", b"").await;
    assert_eq!(header(&resp, "content-type"), "binary/octet-stream");
    assert_eq!(header(&resp, "x-amz-storage-class"), "");
    assert_eq!(text(resp).await, "data");
}

async fn seed(h: &Harness, alice: &Identity) {
    h.call(alice, "PUT", "/bk1", b"").await;
    for k in ["a", "dir/x", "dir/y", "z z", "zz&"] {
        let uri = format!("/bk1/{}", k.replace(' ', "%20").replace('&', "%26"));
        let req = Request::builder().method("PUT").uri(uri).body(Body::from("12345")).unwrap();
        assert_eq!(h.send(alice, req).await.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn list_v1() {
    let h = Harness::new();
    let alice = user("alice");
    seed(&h, &alice).await;

    let body = text(h.call(&alice, "GET", "/bk1?delimiter=/&max-keys=2", b"").await).await;
    assert!(body.contains("<Name>bk1</Name><Prefix/><Marker/><MaxKeys>2</MaxKeys><Delimiter>/</Delimiter><IsTruncated>true</IsTruncated><NextMarker>dir/</NextMarker>"), "{body}");
    assert!(body.contains("<Key>a</Key>"), "{body}");
    assert!(body.contains("<ETag>\"827ccb0eea8a706c4c34a16891f84e7b\"</ETag><Size>5</Size><StorageClass>STANDARD</StorageClass><Owner><ID>alice</ID>"), "{body}");
    assert!(body.contains("<CommonPrefixes><Prefix>dir/</Prefix></CommonPrefixes>"), "{body}");

    let body = text(h.call(&alice, "GET", "/bk1/?delimiter=/&marker=dir/", b"").await).await;
    assert!(body.contains("<IsTruncated>false</IsTruncated>"), "{body}");
    assert!(!body.contains("NextMarker"), "{body}");
    assert!(body.contains("<Key>z z</Key>") && body.contains("<Key>zz&amp;</Key>"), "{body}");

    let body = text(h.call(&alice, "GET", "/bk1?prefix=z&encoding-type=url", b"").await).await;
    assert!(body.contains("<EncodingType>url</EncodingType>"), "{body}");
    assert!(body.contains("<Key>z%20z</Key>") && body.contains("<Key>zz%26</Key>"), "{body}");

    let body = text(h.call(&alice, "GET", "/bk1?max-keys=0", b"").await).await;
    assert!(body.contains("<MaxKeys>0</MaxKeys><IsTruncated>false</IsTruncated></ListBucketResult>"), "{body}");

    let resp = h.call(&alice, "GET", "/bk1?max-keys=-1", b"").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(h.call(&user("bob"), "GET", "/bk1", b"").await.status(), StatusCode::FORBIDDEN);
    assert_eq!(h.call(&alice, "GET", "/nope", b"").await.status(), StatusCode::NOT_FOUND);
    assert_eq!(h.call(&alice, "GET", "/bk1%2Fx", b"").await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_v2_pages() {
    let h = Harness::new();
    let alice = user("alice");
    seed(&h, &alice).await;

    let body = text(h.call(&alice, "GET", "/bk1?list-type=2&max-keys=3", b"").await).await;
    assert!(body.contains("<Name>bk1</Name><Prefix/><NextContinuationToken>dir/y</NextContinuationToken><KeyCount>3</KeyCount><MaxKeys>3</MaxKeys><IsTruncated>true</IsTruncated>"), "{body}");
    assert!(!body.contains("<Owner>"), "{body}");

    let body = text(
        h.call(&alice, "GET", "/bk1?list-type=2&continuation-token=dir%2Fy&fetch-owner=true&start-after=zzz", b"")
            .await,
    )
    .await;
    assert!(body.contains("<StartAfter>zzz</StartAfter><ContinuationToken>dir/y</ContinuationToken><KeyCount>2</KeyCount>"), "{body}");
    assert!(body.contains("<Key>z z</Key>") && body.contains("<Owner><ID>alice</ID>"), "{body}");
    assert!(body.contains("<IsTruncated>false</IsTruncated>"), "{body}");
}

#[tokio::test]
async fn multi_delete() {
    let h = Harness::new();
    let alice = user("alice");
    seed(&h, &alice).await;

    let body: &'static [u8] =
        b"<Delete><Object><Key>a</Key></Object><Object><Key>missing</Key></Object><Object><Key>zz&amp;</Key></Object></Delete>";
    let resp = h.call(&alice, "POST", "/bk1?delete", body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let text_body = text(resp).await;
    assert!(
        text_body.ends_with(
            "<DeleteResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Deleted><Key>a</Key></Deleted><Deleted><Key>missing</Key></Deleted><Deleted><Key>zz&amp;</Key></Deleted></DeleteResult>"
        ),
        "{text_body}"
    );
    assert_eq!(h.call(&alice, "HEAD", "/bk1/a", b"").await.status(), StatusCode::NOT_FOUND);

    let quiet: &'static [u8] = b"<Delete><Quiet>true</Quiet><Object><Key>dir/x</Key></Object></Delete>";
    let text_body = text(h.call(&alice, "POST", "/bk1?delete", quiet).await).await;
    assert!(text_body.ends_with("<DeleteResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"/>"), "{text_body}");

    let resp = h.call(&alice, "POST", "/bk1?delete", b"<Delete></Delete>").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(text(resp).await.contains("<Code>MalformedXML</Code>"));
    assert_eq!(h.call(&user("bob"), "POST", "/bk1?delete", body).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(h.call(&alice, "POST", "/bk1", b"").await.status(), StatusCode::NOT_IMPLEMENTED);
}

/// rgwd nests the admin API at `/admin` beside this router; the two route
/// tables must merge without a conflict panic, and the static prefix wins.
#[tokio::test]
async fn merges_with_nested_admin_router() {
    let h = Harness::new();
    let admin = Router::new().route("/user", axum::routing::get(|| async { "admin" }));
    let app = Router::new().nest("/admin", admin).merge(h.app(user("alice")));
    let req = Request::builder().uri("/admin/user").body(Body::empty()).unwrap();
    assert_eq!(text(app.oneshot(req).await.unwrap()).await, "admin");
}
