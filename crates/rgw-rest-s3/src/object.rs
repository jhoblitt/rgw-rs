//! Object ops: `RGWPutObj`, `RGWGetObj`, `RGWDeleteObj` and
//! `RGWDeleteMultiObj` from `rgw_op.cc`, with the header handling from their
//! `_ObjStore_S3` halves in `rgw_rest_s3.cc`.

use std::io;

use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::TryStreamExt;
use md5::{Digest, Md5};
use rgw_auth::Identity;
use rgw_rest::http_date;
use rgw_sal::{ByteRange, Driver, ObjectBody, body_from_bytes};
use rgw_types::{
    Attrs, ObjectInfo, ObjectKey, Owner, RGW_ATTR_CONTENT_TYPE, RGW_ATTR_META_PREFIX, RGW_ATTR_PREFIX,
    RgwError, RgwResult,
};

use crate::error::{OpError, OpResult, header_value};
use crate::list::{RGW_ATTR_STORAGE_CLASS, storage_class};
use crate::perm::{load_bucket_for, require_user};
use crate::range::{parse_range, resolve_range};
use crate::xml::{DeleteErrorXml, DeleteResult, DeletedXml, XMLNS, parse_delete_request, xml_response};

/// RGW's `Content-Type` when the client sent none (`RGWGetObj_ObjStore_S3`).
pub(crate) const DEFAULT_CONTENT_TYPE: &str = "binary/octet-stream";

/// Largest body buffered to check `Content-MD5`.
const MAX_BUFFERED_PUT: usize = 1 << 30;

/// S3's single-PUT ceiling (`rgw_max_put_size` defaults to 5 GiB).
const MAX_PUT_SIZE: u64 = 5 << 30;

/// Largest `<Delete>` body: 1000 keys of up to 1024 bytes plus markup.
const MAX_MULTI_DELETE_BODY: usize = 4 << 20;

/// `valid_s3_object_name`'s length limit.
const MAX_OBJECT_NAME: usize = 1024;

const AMZ_META: &str = "x-amz-meta-";
const AMZ_STORAGE_CLASS: &str = "x-amz-storage-class";

/// The standard headers RGW persists as `RGW_ATTR_PREFIX + <name>` and
/// echoes on GET (`rgw_to_http_attrs` / `generic_attrs_map` in
/// `rgw_common.cc`).
const GENERIC_ATTRS: &[(HeaderName, &str)] = &[
    (header::CACHE_CONTROL, "cache_control"),
    (header::CONTENT_DISPOSITION, "content_disposition"),
    (header::CONTENT_ENCODING, "content_encoding"),
    (header::CONTENT_LANGUAGE, "content_language"),
    (header::EXPIRES, "expires"),
];

/// `Content-MD5` decoded, or `None` when absent. `InvalidDigest` unless it
/// is base64 of exactly 16 bytes.
fn content_md5(headers: &HeaderMap) -> RgwResult<Option<[u8; 16]>> {
    let Some(v) = headers.get("content-md5") else { return Ok(None) };
    let raw = BASE64.decode(v.as_bytes().trim_ascii()).map_err(|_| RgwError::InvalidDigest)?;
    let digest: [u8; 16] = raw.try_into().map_err(|_| RgwError::InvalidDigest)?;
    Ok(Some(digest))
}

/// Buffer a request body, mapping an oversized one to `EntityTooLarge`.
async fn buffer_body(body: Body, limit: usize) -> RgwResult<bytes::Bytes> {
    to_bytes(body, limit).await.map_err(|e| {
        let inner = e.into_inner();
        if inner.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
            RgwError::EntityTooLarge
        } else {
            RgwError::internal(inner)
        }
    })
}

/// Check a buffered payload against `Content-MD5`, as `RGWPutObj::execute`
/// does before completing the write.
fn verify_md5(expected: [u8; 16], payload: &[u8]) -> RgwResult<()> {
    if Md5::digest(payload).as_slice() == expected { Ok(()) } else { Err(RgwError::BadDigest) }
}

/// The attrs `RGWPutObj::execute` builds from request headers.
fn put_attrs(headers: &HeaderMap) -> Attrs {
    let mut attrs = Attrs::new();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .map(|v| v.as_bytes().to_vec())
        .unwrap_or_else(|| DEFAULT_CONTENT_TYPE.as_bytes().to_vec());
    attrs.insert(RGW_ATTR_CONTENT_TYPE.to_owned(), content_type);

    for (name, value) in headers {
        // `http::HeaderName` is already lowercase.
        if let Some(suffix) = name.as_str().strip_prefix(AMZ_META) {
            // Repeated headers fold into one comma-separated value, as RGW's
            // header map does.
            attrs
                .entry(format!("{RGW_ATTR_META_PREFIX}{suffix}"))
                .and_modify(|v| {
                    v.push(b',');
                    v.extend_from_slice(value.as_bytes());
                })
                .or_insert_with(|| value.as_bytes().to_vec());
        }
    }
    for (name, attr) in GENERIC_ATTRS {
        if let Some(v) = headers.get(name) {
            attrs.insert(format!("{RGW_ATTR_PREFIX}{attr}"), v.as_bytes().to_vec());
        }
    }
    if let Some(sc) = headers.get(AMZ_STORAGE_CLASS) {
        attrs.insert(RGW_ATTR_STORAGE_CLASS.to_owned(), sc.as_bytes().to_vec());
    }
    attrs
}

/// `RGWPutObj`. The body streams to the driver unless `Content-MD5` is
/// present, in which case it is buffered and checked before the driver
/// sees it, so a mismatch stores nothing.
pub(crate) async fn put_object(
    driver: &dyn Driver,
    identity: &Identity,
    bucket_name: &str,
    key: &str,
    headers: &HeaderMap,
    body: Body,
) -> OpResult<Response> {
    if headers.contains_key("x-amz-copy-source") {
        // `RGWCopyObj`.
        return Err(RgwError::NotImplemented.into());
    }
    let bucket = load_bucket_for(driver, identity, bucket_name).await?;
    let user = require_user(identity)?;
    if key.len() > MAX_OBJECT_NAME {
        return Err(RgwError::InvalidArgument("object name too long".into()).into());
    }
    let declared_len = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared_len.is_some_and(|n| n > MAX_PUT_SIZE) {
        return Err(RgwError::EntityTooLarge.into());
    }

    let attrs = put_attrs(headers);
    let payload: ObjectBody = match content_md5(headers)? {
        Some(expected) => {
            let bytes = buffer_body(body, MAX_BUFFERED_PUT).await?;
            verify_md5(expected, &bytes)?;
            body_from_bytes(bytes)
        }
        None => Box::pin(body.into_data_stream().map_err(io::Error::other)),
    };
    let owner = Owner { id: user.user_id.clone(), display_name: user.display_name.clone() };
    let info = driver.put_object(&bucket.bucket, &ObjectKey::new(key), owner, attrs, payload).await?;
    let etag = header_value(&format!("\"{}\"", info.etag))?;
    Ok((StatusCode::OK, [(header::ETAG, etag)]).into_response())
}

/// `416 InvalidRange` with the `Content-Range: bytes */<size>` S3 sends.
fn invalid_range(size: u64) -> OpError {
    let mut err = OpError::from(RgwError::InvalidRange);
    if let Ok(v) = header_value(&format!("bytes */{size}")) {
        err.headers.insert(header::CONTENT_RANGE, v);
    }
    err
}

/// `RGWGetObj_ObjStore_S3::send_response_data`'s headers, then the body.
fn object_response(info: &ObjectInfo, range: Option<(u64, u64)>, body: Body) -> RgwResult<Response> {
    let mut resp = Response::new(body);
    let h = resp.headers_mut();
    let len = match range {
        Some((start, end)) => {
            h.insert(header::CONTENT_RANGE, header_value(&format!("bytes {start}-{end}/{}", info.size))?);
            end - start + 1
        }
        None => info.size,
    };
    h.insert(header::CONTENT_LENGTH, header_value(&len.to_string())?);
    h.insert(header::ETAG, header_value(&format!("\"{}\"", info.etag))?);
    h.insert(header::LAST_MODIFIED, header_value(&http_date(info.mtime))?);
    h.insert(header::CONTENT_TYPE, header_value(info.content_type().unwrap_or(DEFAULT_CONTENT_TYPE))?);
    h.insert(header::ACCEPT_RANGES, header_value("bytes")?);
    for (name, attr) in GENERIC_ATTRS {
        if let Some(v) = info.attrs.get(&format!("{RGW_ATTR_PREFIX}{attr}")) {
            if let Ok(v) = axum::http::HeaderValue::from_bytes(v) {
                h.insert(name, v);
            }
        }
    }
    for (suffix, value) in info.user_metadata() {
        // Stored values came from request headers, but a driver may hold
        // anything; unrepresentable ones are skipped rather than failing
        // the read.
        let name = HeaderName::try_from(format!("{AMZ_META}{suffix}"));
        let value = axum::http::HeaderValue::from_bytes(value);
        if let (Ok(name), Ok(value)) = (name, value) {
            h.append(name, value);
        }
    }
    let sc = storage_class(info);
    if !info.storage_class.is_empty() || info.attrs.contains_key(RGW_ATTR_STORAGE_CLASS) {
        h.insert(AMZ_STORAGE_CLASS, header_value(&sc)?);
    }
    if range.is_some() {
        *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
    }
    Ok(resp)
}

/// `RGWGetObj` with `get_data=true` (GET) or `false` (HEAD).
///
/// HEAD asks the driver for metadata only and resolves any range itself, so
/// it reports the same status and `Content-Range` a GET would.
pub(crate) async fn get_object(
    driver: &dyn Driver,
    identity: &Identity,
    bucket_name: &str,
    key: &str,
    headers: &HeaderMap,
    get_data: bool,
) -> OpResult<Response> {
    let bucket = load_bucket_for(driver, identity, bucket_name).await?;
    let key = ObjectKey::new(key);
    let range: Option<ByteRange> =
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(parse_range);

    if !get_data {
        let info = driver.head_object(&bucket.bucket, &key).await?;
        let resolved = match range {
            Some(r) => Some(resolve_range(r, info.size).ok_or_else(|| invalid_range(info.size))?),
            None => None,
        };
        return Ok(object_response(&info, resolved, Body::empty())?);
    }

    match driver.get_object(&bucket.bucket, &key, range).await {
        Ok(read) => Ok(object_response(&read.info, read.range, Body::from_stream(read.body))?),
        Err(RgwError::InvalidRange) => {
            let size = driver.head_object(&bucket.bucket, &key).await?.size;
            Err(invalid_range(size))
        }
        Err(e) => Err(e.into()),
    }
}

/// `RGWDeleteObj`: 204 whether or not the key existed.
pub(crate) async fn delete_object(
    driver: &dyn Driver,
    identity: &Identity,
    bucket_name: &str,
    key: &str,
) -> OpResult<Response> {
    let bucket = load_bucket_for(driver, identity, bucket_name).await?;
    match driver.delete_object(&bucket.bucket, &ObjectKey::new(key)).await {
        Ok(()) | Err(RgwError::NoSuchKey) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Err(e.into()),
    }
}

/// `RGWDeleteMultiObj` (`POST /bucket?delete`). Keys are deleted one at a
/// time; a missing key is reported as deleted, and any other failure
/// becomes an `<Error>` entry without stopping the batch. `Quiet` drops the
/// `<Deleted>` entries, as `send_partial_response` does.
pub(crate) async fn delete_multi(
    driver: &dyn Driver,
    identity: &Identity,
    bucket_name: &str,
    headers: &HeaderMap,
    body: Body,
) -> OpResult<Response> {
    let bucket = load_bucket_for(driver, identity, bucket_name).await?;
    let expected = content_md5(headers)?;
    let bytes = buffer_body(body, MAX_MULTI_DELETE_BODY).await?;
    if let Some(expected) = expected {
        verify_md5(expected, &bytes)?;
    }
    let req = parse_delete_request(&bytes)?;

    let mut result = DeleteResult { xmlns: XMLNS, deleted: Vec::new(), errors: Vec::new() };
    for obj in req.objects {
        match driver.delete_object(&bucket.bucket, &ObjectKey::new(obj.key.as_str())).await {
            Ok(()) | Err(RgwError::NoSuchKey) => {
                if !req.quiet {
                    result.deleted.push(DeletedXml { key: obj.key });
                }
            }
            Err(e) => result.errors.push(DeleteErrorXml {
                key: obj.key,
                code: e.code().to_owned(),
                message: e.to_string(),
            }),
        }
    }
    Ok(xml_response(&result)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn content_md5_validation() {
        // md5("hello") = 5d41402abc4b2a76b9719d911017c592
        let good = headers(&[("content-md5", "XUFAKrxLKna5cZ2REBfFkg==")]);
        let digest = content_md5(&good).unwrap().unwrap();
        assert_eq!(verify_md5(digest, b"hello"), Ok(()));
        assert_eq!(verify_md5(digest, b"hellO"), Err(RgwError::BadDigest));
        assert_eq!(content_md5(&headers(&[])), Ok(None));
        assert_eq!(content_md5(&headers(&[("content-md5", "!!!")])), Err(RgwError::InvalidDigest));
        assert_eq!(content_md5(&headers(&[("content-md5", "aGVsbG8=")])), Err(RgwError::InvalidDigest));
    }

    #[test]
    fn attrs_from_headers() {
        let h = headers(&[
            ("x-amz-meta-color", "blue"),
            ("x-amz-meta-color", "green"),
            ("x-amz-meta-size", "L"),
            ("x-amz-storage-class", "COLD"),
            ("cache-control", "no-cache"),
        ]);
        let attrs = put_attrs(&h);
        assert_eq!(attrs[RGW_ATTR_CONTENT_TYPE], b"binary/octet-stream");
        assert_eq!(attrs["user.rgw.x-amz-meta-color"], b"blue,green");
        assert_eq!(attrs["user.rgw.x-amz-meta-size"], b"L");
        assert_eq!(attrs[RGW_ATTR_STORAGE_CLASS], b"COLD");
        assert_eq!(attrs["user.rgw.cache_control"], b"no-cache");

        let typed = put_attrs(&headers(&[("content-type", "text/plain")]));
        assert_eq!(typed[RGW_ATTR_CONTENT_TYPE], b"text/plain");
    }
}
