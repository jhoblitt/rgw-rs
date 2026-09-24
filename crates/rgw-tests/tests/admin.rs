//! The admin ops API over HTTP, with requests SigV4-signed by the
//! `aws-sigv4` crate and sent by `reqwest`.

use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use rgw_tests::{TestServer, query_encode as enc};
use serde_json::Value;

/// Send a request signed as the seeded admin; return status and JSON body
/// (`Null` for an empty one).
async fn admin(server: &TestServer, method: &str, path_and_query: &str) -> (u16, Value) {
    let resp = server.admin_request(method, path_and_query).await.unwrap();
    decode(resp).await
}

async fn as_user(server: &TestServer, keys: &(String, String), method: &str, pq: &str) -> (u16, Value) {
    let resp = server.request(method, pq, Some((&keys.0, &keys.1)), &[], Vec::new()).await.unwrap();
    decode(resp).await
}

async fn decode(resp: reqwest::Response) -> (u16, Value) {
    let status = resp.status().as_u16();
    let body = resp.bytes().await.unwrap();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("{status}: not JSON ({e}): {}", String::from_utf8_lossy(&body)))
    };
    (status, value)
}

/// Create a user over the API with one generated key; returns its keys.
async fn api_create_user(server: &TestServer, uid: &str) -> (String, String) {
    let (status, doc) =
        admin(server, "PUT", &format!("/admin/user?uid={}&display-name={}&generate-key=true", enc(uid), enc(uid)))
            .await;
    assert_eq!(status, 200, "{doc}");
    let key = &doc["keys"][0];
    (key["access_key"].as_str().unwrap().to_owned(), key["secret_key"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn info() {
    let server = TestServer::spawn().await.unwrap();
    let (status, doc) = admin(&server, "GET", "/admin/info").await;
    assert_eq!(status, 200, "{doc}");
    let backends = doc["info"]["storage_backends"].as_array().unwrap();
    assert_eq!(backends.len(), 1, "{doc}");
    assert!(backends[0]["name"].is_string(), "{doc}");
}

#[tokio::test]
async fn user_create_info_and_list() {
    let server = TestServer::spawn().await.unwrap();
    let (status, doc) = admin(&server, "PUT", "/admin/user?uid=alice&display-name=Alice%20A&generate-key=true").await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["user_id"], "alice");
    assert_eq!(doc["display_name"], "Alice A");
    let keys = doc["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1, "{doc}");
    assert_eq!(keys[0]["user"], "alice");
    assert_eq!(keys[0]["access_key"].as_str().unwrap().len(), 20, "{doc}");
    assert_eq!(keys[0]["secret_key"].as_str().unwrap().len(), 40, "{doc}");
    assert!(doc["caps"].as_array().unwrap().is_empty(), "{doc}");
    assert!(doc["create_date"].as_str().unwrap().ends_with('Z'), "{doc}");
    assert_eq!(doc["suspended"], 0);

    let (status, info) = admin(&server, "GET", "/admin/user?uid=alice").await;
    assert_eq!(status, 200, "{info}");
    assert_eq!(info["user_id"], "alice");
    assert_eq!(info["keys"], doc["keys"]);

    let (status, list) = admin(&server, "GET", "/admin/user?list").await;
    assert_eq!(status, 200, "{list}");
    let ids: Vec<&str> = list["keys"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(ids, ["admin", "alice"], "{list}");
    assert_eq!(list["count"], 2);
    assert_eq!(list["truncated"], false);

    let (status, err) = admin(&server, "GET", "/admin/user?uid=nobody").await;
    assert_eq!((status, &err["Code"]), (404, &Value::from("NoSuchUser")), "{err}");
}

#[tokio::test]
async fn key_add_and_remove() {
    let server = TestServer::spawn().await.unwrap();
    api_create_user(&server, "bob").await;
    let (status, keys) =
        admin(&server, "PUT", "/admin/user?key&uid=bob&access-key=BOBKEY0000000000000X&secret-key=bobsecret").await;
    assert_eq!(status, 200, "{keys}");
    assert!(keys.as_array().unwrap().iter().any(|k| k["access_key"] == "BOBKEY0000000000000X"), "{keys}");

    let s3 = server.s3_client_for("BOBKEY0000000000000X", "bobsecret");
    s3.list_buckets().send().await.unwrap();

    let (status, body) = admin(&server, "DELETE", "/admin/user?key&uid=bob&access-key=BOBKEY0000000000000X").await;
    assert_eq!(status, 200, "{body}");
    let err = s3.list_buckets().send().await.unwrap_err();
    assert_eq!(err.code(), Some("InvalidAccessKeyId"), "{err:?}");
}

#[tokio::test]
async fn caps_grant_read_but_not_write() {
    let server = TestServer::spawn().await.unwrap();
    let keys = api_create_user(&server, "carol").await;

    let (status, err) = as_user(&server, &keys, "GET", "/admin/bucket").await;
    assert_eq!((status, &err["Code"]), (403, &Value::from("AccessDenied")), "{err}");

    let (status, caps) = admin(&server, "PUT", &format!("/admin/user?caps&uid=carol&user-caps={}", enc("buckets=read"))).await;
    assert_eq!(status, 200, "{caps}");
    assert_eq!(caps, serde_json::json!([{"type": "buckets", "perm": "read"}]));

    let (status, list) = as_user(&server, &keys, "GET", "/admin/bucket").await;
    assert_eq!(status, 200, "{list}");
    assert!(list.is_array(), "{list}");

    let (status, err) = as_user(&server, &keys, "DELETE", "/admin/bucket?bucket=whatever").await;
    assert_eq!((status, &err["Code"]), (403, &Value::from("AccessDenied")), "{err}");
}

#[tokio::test]
async fn suspended_user_is_refused() {
    let server = TestServer::spawn().await.unwrap();
    let keys = api_create_user(&server, "dave").await;
    let s3 = server.s3_client_for(&keys.0, &keys.1);
    s3.list_buckets().send().await.unwrap();

    let (status, doc) = admin(&server, "POST", "/admin/user?uid=dave&suspended=true").await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["suspended"], 1);

    let err = s3.list_buckets().send().await.unwrap_err();
    assert_eq!(err.code(), Some("UserSuspended"), "{err:?}");
    assert_eq!(err.raw_response().map(|r| r.status().as_u16()), Some(403));

    admin(&server, "POST", "/admin/user?uid=dave&suspended=false").await;
    s3.list_buckets().send().await.unwrap();
}

#[tokio::test]
async fn bucket_stats_list_and_purge() {
    let server = TestServer::spawn().await.unwrap();
    let keys = api_create_user(&server, "erin").await;
    let s3 = server.s3_client_for(&keys.0, &keys.1);
    for b in ["erin-one", "erin-two"] {
        s3.create_bucket().bucket(b).send().await.unwrap();
    }
    let body = b"twelve bytes".to_vec();
    s3.put_object().bucket("erin-one").key("k").body(ByteStream::from(body.clone())).send().await.unwrap();

    let (status, doc) = admin(&server, "GET", "/admin/bucket?bucket=erin-one&stats=true").await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["bucket"], "erin-one");
    assert_eq!(doc["owner"], "erin");
    assert_eq!(doc["usage"]["rgw.main"]["num_objects"], 1, "{doc}");
    assert_eq!(doc["usage"]["rgw.main"]["size"], body.len(), "{doc}");

    let (status, names) = admin(&server, "GET", "/admin/bucket?uid=erin").await;
    assert_eq!(status, 200, "{names}");
    assert_eq!(names, serde_json::json!(["erin-one", "erin-two"]));

    let (status, body) = admin(&server, "DELETE", "/admin/bucket?bucket=erin-one").await;
    assert_eq!((status, &body["Code"]), (409, &Value::from("BucketNotEmpty")), "{body}");
    let (status, body) = admin(&server, "DELETE", "/admin/bucket?bucket=erin-one&purge-objects=true").await;
    assert_eq!(status, 200, "{body}");
    let err = s3.head_bucket().bucket("erin-one").send().await.unwrap_err();
    assert!(err.as_service_error().is_some_and(|e| e.is_not_found()), "{err:?}");
    let listed = s3.list_buckets().send().await.unwrap();
    let names: Vec<_> = listed.buckets().iter().filter_map(|b| b.name()).collect();
    assert_eq!(names, ["erin-two"]);
}

#[tokio::test]
async fn user_remove_purges_data() {
    let server = TestServer::spawn().await.unwrap();
    let keys = api_create_user(&server, "frank").await;
    let s3 = server.s3_client_for(&keys.0, &keys.1);
    s3.create_bucket().bucket("frank-data").send().await.unwrap();
    s3.put_object().bucket("frank-data").key("k").body(ByteStream::from_static(b"x")).send().await.unwrap();

    let (status, body) = admin(&server, "DELETE", "/admin/user?uid=frank&purge-data=true").await;
    assert_eq!(status, 200, "{body}");
    let (status, err) = admin(&server, "GET", "/admin/user?uid=frank").await;
    assert_eq!((status, &err["Code"]), (404, &Value::from("NoSuchUser")), "{err}");
    let (status, err) = admin(&server, "GET", "/admin/bucket?bucket=frank-data").await;
    assert_eq!((status, &err["Code"]), (404, &Value::from("NoSuchBucket")), "{err}");
    let err = s3.list_buckets().send().await.unwrap_err();
    assert_eq!(err.code(), Some("InvalidAccessKeyId"), "{err:?}");
}

#[tokio::test]
async fn user_without_caps_is_denied() {
    let server = TestServer::spawn().await.unwrap();
    let keys = api_create_user(&server, "grace").await;
    let (status, err) = as_user(&server, &keys, "GET", "/admin/user?uid=admin").await;
    assert_eq!(status, 403, "{err}");
    assert_eq!(err["Code"], "AccessDenied", "{err}");
}

#[tokio::test]
async fn anonymous_is_denied() {
    let server = TestServer::spawn().await.unwrap();
    let resp = server.request("GET", "/admin/user?uid=admin", None, &[], Vec::new()).await.unwrap();
    let (status, err) = decode(resp).await;
    assert_eq!(status, 403, "{err}");
    assert_eq!(err["Code"], "AccessDenied", "{err}");
}
