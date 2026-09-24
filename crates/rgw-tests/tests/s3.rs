//! The S3 API driven by the real AWS SDK for Rust against an in-process
//! `rgwd`.

use std::collections::BTreeSet;

use aws_sdk_s3::Client;
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use rgw_tests::{SignableBody, TestServer};

/// The S3 `<Code>` an SDK call failed with.
fn code<E: ProvideErrorMetadata + std::fmt::Debug, R: std::fmt::Debug>(err: &SdkError<E, R>) -> String {
    err.code().map(str::to_owned).unwrap_or_else(|| format!("no code in {err:?}"))
}

async fn setup() -> (TestServer, Client) {
    let server = TestServer::spawn().await.unwrap();
    let client = server.admin_s3_client();
    (server, client)
}

async fn put(client: &Client, bucket: &str, key: &str, body: &[u8]) -> String {
    let out = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(body.to_vec()))
        .send()
        .await
        .unwrap_or_else(|e| panic!("put {key}: {e:?}"));
    out.e_tag().unwrap().to_owned()
}

async fn get_bytes(client: &Client, bucket: &str, key: &str) -> Vec<u8> {
    let out = client.get_object().bucket(bucket).key(key).send().await.unwrap();
    out.body.collect().await.unwrap().into_bytes().to_vec()
}

#[tokio::test]
async fn create_list_and_head_bucket() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("alpha").send().await.unwrap();

    let list = client.list_buckets().send().await.unwrap();
    let names: Vec<_> = list.buckets().iter().filter_map(|b| b.name()).collect();
    assert_eq!(names, ["alpha"]);
    assert!(list.buckets()[0].creation_date().is_some());

    client.head_bucket().bucket("alpha").send().await.unwrap();
    let err = client.head_bucket().bucket("missing").send().await.unwrap_err();
    assert!(err.as_service_error().is_some_and(|e| e.is_not_found()), "{err:?}");
}

#[tokio::test]
async fn put_head_get_object_with_metadata() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let body = b"hello, rados gateway".to_vec();
    let put = client
        .put_object()
        .bucket("bkt")
        .key("dir/hello.txt")
        .content_type("text/plain")
        .metadata("color", "blue")
        .metadata("shape", "round")
        .body(ByteStream::from(body.clone()))
        .send()
        .await
        .unwrap();
    let etag = put.e_tag().unwrap().to_owned();
    assert!(etag.starts_with('"') && etag.ends_with('"') && etag.len() == 34, "{etag}");

    let head = client.head_object().bucket("bkt").key("dir/hello.txt").send().await.unwrap();
    assert_eq!(head.e_tag(), Some(etag.as_str()));
    assert_eq!(head.content_length(), Some(body.len() as i64));
    assert_eq!(head.content_type(), Some("text/plain"));
    let meta = head.metadata().unwrap();
    assert_eq!(meta.get("color").map(String::as_str), Some("blue"));
    assert_eq!(meta.get("shape").map(String::as_str), Some("round"));
    assert_eq!(meta.len(), 2, "{meta:?}");
    assert!(head.last_modified().is_some());

    let get = client.get_object().bucket("bkt").key("dir/hello.txt").send().await.unwrap();
    assert_eq!(get.e_tag(), Some(etag.as_str()));
    assert_eq!(get.content_type(), Some("text/plain"));
    assert_eq!(get.metadata().unwrap().get("color").map(String::as_str), Some("blue"));
    assert_eq!(get.body.collect().await.unwrap().into_bytes().as_ref(), body.as_slice());
}

#[tokio::test]
async fn ranged_get() {
    let (server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let body: Vec<u8> = (0u8..=99).collect();
    put(&client, "bkt", "k", &body).await;

    let get = client.get_object().bucket("bkt").key("k").range("bytes=10-19").send().await.unwrap();
    assert_eq!(get.content_range(), Some("bytes 10-19/100"));
    assert_eq!(get.content_length(), Some(10));
    assert_eq!(get.body.collect().await.unwrap().into_bytes().as_ref(), &body[10..20]);

    let get = client.get_object().bucket("bkt").key("k").range("bytes=-5").send().await.unwrap();
    assert_eq!(get.body.collect().await.unwrap().into_bytes().as_ref(), &body[95..]);

    // The SDK hides the status line; check the 206 on the wire.
    let (a, s) = server.admin_keys.clone();
    let resp = server.request("GET", "/bkt/k", Some((&a, &s)), &[("range", "bytes=0-9")], Vec::new()).await.unwrap();
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.headers()["content-range"], "bytes 0-9/100");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), &body[..10]);
}

#[tokio::test]
async fn unsatisfiable_range_is_invalid_range() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    put(&client, "bkt", "k", b"0123456789").await;
    let err = client.get_object().bucket("bkt").key("k").range("bytes=100-200").send().await.unwrap_err();
    assert_eq!(code(&err), "InvalidRange");
    assert_eq!(err.raw_response().map(|r| r.status().as_u16()), Some(416));
}

#[tokio::test]
async fn list_objects_v2_pages_with_delimiter() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let keys = [
        "photos/a.jpg",
        "photos/b.jpg",
        "photos/2024/c.jpg",
        "photos/2024/d.jpg",
        "photos/2025/e.jpg",
        "photos/z.jpg",
        "other/x",
    ];
    for k in keys {
        put(&client, "bkt", k, k.as_bytes()).await;
    }

    let mut objects = Vec::new();
    let mut prefixes = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let out = client
            .list_objects_v2()
            .bucket("bkt")
            .prefix("photos/")
            .delimiter("/")
            .max_keys(2)
            .set_continuation_token(token.clone())
            .send()
            .await
            .unwrap();
        pages += 1;
        assert!(out.key_count().unwrap() <= 2);
        objects.extend(out.contents().iter().map(|o| o.key().unwrap().to_owned()));
        prefixes.extend(out.common_prefixes().iter().map(|p| p.prefix().unwrap().to_owned()));
        if !out.is_truncated().unwrap_or(false) {
            break;
        }
        token = Some(out.next_continuation_token().expect("truncated page without a token").to_owned());
        assert!(pages < 10, "listing does not terminate");
    }
    assert_eq!(objects, ["photos/a.jpg", "photos/b.jpg", "photos/z.jpg"]);
    assert_eq!(prefixes, ["photos/2024/", "photos/2025/"]);
    assert_eq!(pages, 3);

    // Without a delimiter everything under the prefix comes back.
    let out = client.list_objects_v2().bucket("bkt").prefix("photos/2024/").send().await.unwrap();
    let got: Vec<_> = out.contents().iter().filter_map(|o| o.key()).collect();
    assert_eq!(got, ["photos/2024/c.jpg", "photos/2024/d.jpg"]);
    assert_eq!(out.contents()[0].size(), Some("photos/2024/c.jpg".len() as i64));
}

#[tokio::test]
async fn list_objects_v1_pages_with_marker() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let keys: BTreeSet<String> = (0..7).map(|i| format!("key-{i:02}")).collect();
    for k in &keys {
        put(&client, "bkt", k, b"x").await;
    }

    let mut seen = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let out = client
            .list_objects()
            .bucket("bkt")
            .max_keys(3)
            .set_marker(marker.clone())
            .send()
            .await
            .unwrap();
        let page: Vec<String> = out.contents().iter().map(|o| o.key().unwrap().to_owned()).collect();
        assert!(page.len() <= 3);
        seen.extend(page.iter().cloned());
        if !out.is_truncated().unwrap_or(false) {
            break;
        }
        // Without a delimiter S3 may omit NextMarker; the last key is the
        // marker then.
        marker = Some(out.next_marker().map(str::to_owned).unwrap_or_else(|| page.last().unwrap().clone()));
        assert!(seen.len() < 20, "listing does not terminate");
    }
    assert_eq!(seen, keys.into_iter().collect::<Vec<_>>());
}

#[tokio::test]
async fn overwrite_changes_etag() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let first = put(&client, "bkt", "k", b"first").await;
    let second = put(&client, "bkt", "k", b"second version").await;
    assert_ne!(first, second);
    let head = client.head_object().bucket("bkt").key("k").send().await.unwrap();
    assert_eq!(head.e_tag(), Some(second.as_str()));
    assert_eq!(get_bytes(&client, "bkt", "k").await, b"second version");
}

#[tokio::test]
async fn delete_object_then_head_is_not_found() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    put(&client, "bkt", "k", b"x").await;
    client.delete_object().bucket("bkt").key("k").send().await.unwrap();
    let err = client.head_object().bucket("bkt").key("k").send().await.unwrap_err();
    assert!(err.as_service_error().is_some_and(|e| e.is_not_found()), "{err:?}");
    // Deleting a missing key succeeds, as in S3.
    client.delete_object().bucket("bkt").key("k").send().await.unwrap();
    let err = client.get_object().bucket("bkt").key("k").send().await.unwrap_err();
    assert!(err.as_service_error().is_some_and(|e| e.is_no_such_key()), "{err:?}");
}

#[tokio::test]
async fn delete_objects_removes_several_keys() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    for k in ["a", "b", "c", "keep"] {
        put(&client, "bkt", k, b"x").await;
    }
    let delete = Delete::builder()
        .set_objects(Some(
            ["a", "b", "c"].iter().map(|k| ObjectIdentifier::builder().key(*k).build().unwrap()).collect(),
        ))
        .build()
        .unwrap();
    let out = client.delete_objects().bucket("bkt").delete(delete).send().await.unwrap();
    let mut deleted: Vec<_> = out.deleted().iter().filter_map(|d| d.key()).collect();
    deleted.sort_unstable();
    assert_eq!(deleted, ["a", "b", "c"]);
    assert!(out.errors().is_empty(), "{:?}", out.errors());

    let out = client.list_objects_v2().bucket("bkt").send().await.unwrap();
    let left: Vec<_> = out.contents().iter().filter_map(|o| o.key()).collect();
    assert_eq!(left, ["keep"]);
}

#[tokio::test]
async fn delete_non_empty_bucket() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    put(&client, "bkt", "k1", b"x").await;
    put(&client, "bkt", "k2", b"y").await;
    let err = client.delete_bucket().bucket("bkt").send().await.unwrap_err();
    assert_eq!(code(&err), "BucketNotEmpty");

    for k in ["k1", "k2"] {
        client.delete_object().bucket("bkt").key(k).send().await.unwrap();
    }
    client.delete_bucket().bucket("bkt").send().await.unwrap();
    let list = client.list_buckets().send().await.unwrap();
    assert!(list.buckets().is_empty());
}

#[tokio::test]
async fn other_user_is_denied() {
    let (server, client) = setup().await;
    client.create_bucket().bucket("private").send().await.unwrap();
    put(&client, "private", "secret", b"mine").await;

    let (a, s) = server.create_user("mallory", "").await.unwrap();
    let other = server.s3_client_for(&a, &s);
    let err = other.get_object().bucket("private").key("secret").send().await.unwrap_err();
    assert_eq!(code(&err), "AccessDenied");
    let err = other.list_objects_v2().bucket("private").send().await.unwrap_err();
    assert_eq!(code(&err), "AccessDenied");
    // Her own listing does not show the bucket.
    assert!(other.list_buckets().send().await.unwrap().buckets().is_empty());
}

#[tokio::test]
async fn wrong_secret_is_signature_mismatch() {
    let (server, _client) = setup().await;
    let bad = server.s3_client_for(&server.admin_keys.0, "not-the-secret");
    let err = bad.list_buckets().send().await.unwrap_err();
    assert_eq!(code(&err), "SignatureDoesNotMatch");
}

#[tokio::test]
async fn unknown_access_key_is_invalid() {
    let (server, _client) = setup().await;
    let bad = server.s3_client_for("NOSUCHACCESSKEY00000", "whatever");
    let err = bad.list_buckets().send().await.unwrap_err();
    assert_eq!(code(&err), "InvalidAccessKeyId");
}

#[tokio::test]
async fn unsigned_request_is_access_denied() {
    let (server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let resp = reqwest::get(format!("{}/bkt", server.endpoint())).await.unwrap();
    assert_eq!(resp.status(), 403);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap().to_owned();
    assert_eq!(ct, "application/xml");
    let body = resp.text().await.unwrap();
    assert!(body.contains("<Code>AccessDenied</Code>"), "{body}");
}

/// A file-backed body makes the SDK sign `aws-chunked` with a trailing
/// CRC32 (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`), where an in-memory
/// one is sent whole with a hex payload hash and a checksum header.
#[tokio::test]
async fn put_from_file_uses_signed_chunked_trailer() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blob");
    let body: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    std::fs::write(&path, &body).unwrap();

    let stream = ByteStream::from_path(&path).await.unwrap();
    let put = client.put_object().bucket("bkt").key("blob").body(stream).send().await.unwrap();
    let head = client.head_object().bucket("bkt").key("blob").send().await.unwrap();
    assert_eq!(head.e_tag(), put.e_tag());
    assert_eq!(head.content_length(), Some(body.len() as i64));
    // The aws-chunked framing must not leak into the stored object.
    assert_eq!(head.content_encoding(), None);
    assert_eq!(get_bytes(&client, "bkt", "blob").await, body);
}

/// `STREAMING-UNSIGNED-PAYLOAD-TRAILER` is what SDKs send over HTTPS; no
/// client uses it against a plain-HTTP endpoint, so the framing is built
/// by hand here and the headers are signed by `aws-sigv4`.
async fn put_unsigned_trailer(server: &TestServer, key: &str, chunks: &[&[u8]], crc: u32) -> reqwest::Response {
    use base64::Engine as _;
    let payload_len: usize = chunks.iter().map(|c| c.len()).sum();
    let mut body = Vec::new();
    for chunk in chunks {
        body.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        body.extend_from_slice(chunk);
        body.extend_from_slice(b"\r\n");
    }
    let crc = base64::engine::general_purpose::STANDARD.encode(crc.to_be_bytes());
    body.extend_from_slice(format!("0\r\nx-amz-checksum-crc32:{crc}\r\n\r\n").as_bytes());

    let url = format!("{}/bkt/{key}", server.endpoint());
    let (a, s) = &server.admin_keys;
    let signed = rgw_tests::sigv4_headers("PUT", &url, a, s, SignableBody::StreamingUnsignedPayloadTrailer).unwrap();
    let mut req = reqwest::Client::new()
        .put(&url)
        .header("content-encoding", "aws-chunked")
        .header("x-amz-decoded-content-length", payload_len.to_string())
        .header("x-amz-trailer", "x-amz-checksum-crc32");
    for (name, value) in signed {
        req = req.header(name, value);
    }
    req.body(body).send().await.unwrap()
}

#[tokio::test]
async fn put_unsigned_payload_trailer() {
    let (server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let chunks: [&[u8]; 2] = [b"hello ", b"trailer"];
    let resp = put_unsigned_trailer(&server, "k", &chunks, crc32fast::hash(b"hello trailer")).await;
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
    assert_eq!(get_bytes(&client, "bkt", "k").await, b"hello trailer");

    let resp = put_unsigned_trailer(&server, "bad", &chunks, 0xdead_beef).await;
    assert_eq!(resp.status(), 400);
    assert!(resp.text().await.unwrap().contains("<Code>BadDigest</Code>"));
    let err = client.head_object().bucket("bkt").key("bad").send().await.unwrap_err();
    assert!(err.as_service_error().is_some_and(|e| e.is_not_found()), "{err:?}");
}

/// Keys that exercise canonical-URI encoding and XML escaping, plus an
/// empty object.
#[tokio::test]
async fn awkward_keys_and_empty_object() {
    let (_server, client) = setup().await;
    client.create_bucket().bucket("bkt").send().await.unwrap();
    let keys = ["a b/c+d.txt", "pct%20lit", "q?x=1&y", "tilde~x", "xml<&>'\"", "ünï/cødé", "empty"];
    for k in keys {
        let body: &[u8] = if k == "empty" { b"" } else { k.as_bytes() };
        put(&client, "bkt", k, body).await;
        assert_eq!(get_bytes(&client, "bkt", k).await, body, "{k}");
    }
    let head = client.head_object().bucket("bkt").key("empty").send().await.unwrap();
    assert_eq!(head.content_length(), Some(0));
    assert_eq!(head.e_tag(), Some("\"d41d8cd98f00b204e9800998ecf8427e\""));

    let mut want: Vec<&str> = keys.to_vec();
    want.sort_unstable();
    let out = client.list_objects_v2().bucket("bkt").send().await.unwrap();
    let got: Vec<_> = out.contents().iter().filter_map(|o| o.key()).collect();
    assert_eq!(got, want);
    let out = client.list_objects().bucket("bkt").send().await.unwrap();
    let got: Vec<_> = out.contents().iter().filter_map(|o| o.key()).collect();
    assert_eq!(got, want);

    let out = client.list_objects_v2().bucket("bkt").delimiter("/").send().await.unwrap();
    let prefixes: Vec<_> = out.common_prefixes().iter().filter_map(|p| p.prefix()).collect();
    assert_eq!(prefixes, ["a b/", "ünï/"]);

    let delete = Delete::builder()
        .set_objects(Some(keys.iter().map(|k| ObjectIdentifier::builder().key(*k).build().unwrap()).collect()))
        .build()
        .unwrap();
    let out = client.delete_objects().bucket("bkt").delete(delete).send().await.unwrap();
    assert_eq!(out.deleted().len(), keys.len(), "{:?}", out.errors());
    client.delete_bucket().bucket("bkt").send().await.unwrap();
}
