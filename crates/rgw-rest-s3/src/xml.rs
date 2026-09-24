//! The XML documents: what each op's `send_response` in `rgw_rest_s3.cc`
//! writes through RGW's `XMLFormatter`, as serde structs, plus the
//! `RGWMultiDelDelete` request parser from `rgw_multi_del.cc`.
//!
//! Field order is element order on the wire.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use rgw_types::{RgwError, RgwResult};

/// `XMLNS_AWS_S3` in `rgw_rest_s3.h`.
pub(crate) const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

/// Serialize a document with the XML declaration RGW's `dump_start` emits.
pub(crate) fn to_xml<T: Serialize>(doc: &T) -> RgwResult<String> {
    let body = quick_xml::se::to_string(doc).map_err(RgwError::internal)?;
    Ok(format!("{XML_DECL}{body}"))
}

/// A 200 reply carrying `doc`.
pub(crate) fn xml_response<T: Serialize>(doc: &T) -> RgwResult<Response> {
    let body = to_xml(doc)?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "application/xml")], body).into_response())
}

/// `ACLOwner::dump_xml` / `dump_owner`.
#[derive(Debug, Serialize)]
pub(crate) struct OwnerXml {
    #[serde(rename = "ID")]
    pub(crate) id: String,
    #[serde(rename = "DisplayName")]
    pub(crate) display_name: String,
}

/// `RGWListBuckets_ObjStore_S3::send_response_begin` / `_data` / `_end`.
#[derive(Debug, Serialize)]
#[serde(rename = "ListAllMyBucketsResult")]
pub(crate) struct ListAllMyBucketsResult {
    #[serde(rename = "@xmlns")]
    pub(crate) xmlns: &'static str,
    #[serde(rename = "Owner")]
    pub(crate) owner: OwnerXml,
    #[serde(rename = "Buckets")]
    pub(crate) buckets: BucketsXml,
}

#[derive(Debug, Serialize)]
pub(crate) struct BucketsXml {
    #[serde(rename = "Bucket")]
    pub(crate) bucket: Vec<BucketXml>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BucketXml {
    #[serde(rename = "Name")]
    pub(crate) name: String,
    #[serde(rename = "CreationDate")]
    pub(crate) creation_date: String,
}

/// One `<Contents>` entry of either listing version.
#[derive(Debug, Serialize)]
pub(crate) struct ContentsXml {
    #[serde(rename = "Key")]
    pub(crate) key: String,
    #[serde(rename = "LastModified")]
    pub(crate) last_modified: String,
    /// Quoted, as `dump_format("ETag", "\"%s\"")` writes it.
    #[serde(rename = "ETag")]
    pub(crate) etag: String,
    #[serde(rename = "Size")]
    pub(crate) size: u64,
    #[serde(rename = "StorageClass")]
    pub(crate) storage_class: String,
    #[serde(rename = "Owner", skip_serializing_if = "Option::is_none")]
    pub(crate) owner: Option<OwnerXml>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommonPrefixXml {
    #[serde(rename = "Prefix")]
    pub(crate) prefix: String,
}

/// `RGWListBucket_ObjStore_S3::send_response` (ListObjects V1).
#[derive(Debug, Serialize)]
#[serde(rename = "ListBucketResult")]
pub(crate) struct ListBucketResultV1 {
    #[serde(rename = "@xmlns")]
    pub(crate) xmlns: &'static str,
    #[serde(rename = "Name")]
    pub(crate) name: String,
    #[serde(rename = "Prefix")]
    pub(crate) prefix: String,
    #[serde(rename = "Marker")]
    pub(crate) marker: String,
    #[serde(rename = "MaxKeys")]
    pub(crate) max_keys: usize,
    #[serde(rename = "Delimiter", skip_serializing_if = "Option::is_none")]
    pub(crate) delimiter: Option<String>,
    #[serde(rename = "EncodingType", skip_serializing_if = "Option::is_none")]
    pub(crate) encoding_type: Option<&'static str>,
    #[serde(rename = "IsTruncated")]
    pub(crate) is_truncated: bool,
    #[serde(rename = "NextMarker", skip_serializing_if = "Option::is_none")]
    pub(crate) next_marker: Option<String>,
    #[serde(rename = "Contents")]
    pub(crate) contents: Vec<ContentsXml>,
    #[serde(rename = "CommonPrefixes")]
    pub(crate) common_prefixes: Vec<CommonPrefixXml>,
}

/// `RGWListBucket_ObjStore_S3v2::send_response` (ListObjectsV2).
#[derive(Debug, Serialize)]
#[serde(rename = "ListBucketResult")]
pub(crate) struct ListBucketResultV2 {
    #[serde(rename = "@xmlns")]
    pub(crate) xmlns: &'static str,
    #[serde(rename = "Name")]
    pub(crate) name: String,
    #[serde(rename = "Prefix")]
    pub(crate) prefix: String,
    #[serde(rename = "StartAfter", skip_serializing_if = "Option::is_none")]
    pub(crate) start_after: Option<String>,
    #[serde(rename = "ContinuationToken", skip_serializing_if = "Option::is_none")]
    pub(crate) continuation_token: Option<String>,
    #[serde(rename = "NextContinuationToken", skip_serializing_if = "Option::is_none")]
    pub(crate) next_continuation_token: Option<String>,
    #[serde(rename = "KeyCount")]
    pub(crate) key_count: usize,
    #[serde(rename = "MaxKeys")]
    pub(crate) max_keys: usize,
    #[serde(rename = "Delimiter", skip_serializing_if = "Option::is_none")]
    pub(crate) delimiter: Option<String>,
    #[serde(rename = "EncodingType", skip_serializing_if = "Option::is_none")]
    pub(crate) encoding_type: Option<&'static str>,
    #[serde(rename = "IsTruncated")]
    pub(crate) is_truncated: bool,
    #[serde(rename = "Contents")]
    pub(crate) contents: Vec<ContentsXml>,
    #[serde(rename = "CommonPrefixes")]
    pub(crate) common_prefixes: Vec<CommonPrefixXml>,
}

/// `RGWGetBucketLocation_ObjStore_S3::send_response`.
#[derive(Debug, Serialize)]
#[serde(rename = "LocationConstraint")]
pub(crate) struct LocationConstraint {
    #[serde(rename = "@xmlns")]
    pub(crate) xmlns: &'static str,
    #[serde(rename = "$text")]
    pub(crate) location: String,
}

/// `RGWGetBucketVersioning_ObjStore_S3::send_response` for a bucket that
/// never had versioning enabled: no `<Status>`.
#[derive(Debug, Serialize)]
#[serde(rename = "VersioningConfiguration")]
pub(crate) struct VersioningConfiguration {
    #[serde(rename = "@xmlns")]
    pub(crate) xmlns: &'static str,
}

/// `RGWMultiDelDelete`: the `DeleteObjects` request body.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DeleteRequest {
    pub(crate) objects: Vec<DeleteObject>,
    pub(crate) quiet: bool,
}

/// `RGWMultiDelObject`. `VersionId` is accepted and ignored: buckets here
/// are never versioned.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeleteObject {
    pub(crate) key: String,
    pub(crate) version_id: Option<String>,
}

/// `rgw_delete_multi_obj_max_num`'s default.
pub(crate) const MAX_MULTI_DELETE: usize = 1000;

/// Parse a `<Delete>` body. An empty object list, an `<Object>` without a
/// `<Key>`, or more than [`MAX_MULTI_DELETE`] objects is `MalformedXML`, as
/// in `RGWDeleteMultiObj::execute`.
///
/// This walks the events by hand because quick-xml's serde deserializer
/// trims whitespace from text, and object keys may begin or end with
/// spaces.
pub(crate) fn parse_delete_request(body: &[u8]) -> RgwResult<DeleteRequest> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let malformed = |_| RgwError::MalformedXml;
    let text = std::str::from_utf8(body).map_err(malformed)?;
    let mut reader = Reader::from_str(text);
    let mut req = DeleteRequest::default();
    let mut path: Vec<String> = Vec::new();
    let mut content = String::new();
    let mut key: Option<String> = None;
    let mut version_id: Option<String> = None;
    let mut saw_root = false;

    loop {
        let event = reader.read_event().map_err(|_| RgwError::MalformedXml)?;
        let (open, close) = match &event {
            Event::Start(e) => (Some(e.local_name().as_ref().to_owned()), false),
            Event::Empty(e) => (Some(e.local_name().as_ref().to_owned()), true),
            Event::End(_) => (None, true),
            Event::Text(t) => {
                content.push_str(&t);
                continue;
            }
            Event::CData(c) => {
                content.push_str(&c);
                continue;
            }
            // quick-xml hands `&amp;`-style references over unresolved; the
            // five predefined entities are all a well-formed document may use
            // without a DTD.
            Event::GeneralRef(r) => {
                match r.resolve_char_ref().map_err(|_| RgwError::MalformedXml)? {
                    Some(ch) => content.push(ch),
                    None => content.push_str(match r.borrow().into_inner().as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => return Err(RgwError::MalformedXml),
                    }),
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        if let Some(name) = open {
            if path.is_empty() {
                if name != "Delete" || saw_root {
                    return Err(RgwError::MalformedXml);
                }
                saw_root = true;
            }
            if path.len() == 1 && name == "Object" {
                key = None;
                version_id = None;
            }
            path.push(name);
            content.clear();
        }
        if close {
            let taken = std::mem::take(&mut content);
            let elems: Vec<&str> = path.iter().map(String::as_str).collect();
            match elems.as_slice() {
                ["Delete", "Object", "Key"] => key = Some(taken),
                ["Delete", "Object", "VersionId"] => version_id = Some(taken),
                ["Delete", "Object"] => {
                    let key = key.take().ok_or(RgwError::MalformedXml)?;
                    req.objects.push(DeleteObject { key, version_id: version_id.take() });
                }
                ["Delete", "Quiet"] => req.quiet = taken.trim().eq_ignore_ascii_case("true"),
                _ => {}
            }
            path.pop();
        }
    }
    if !saw_root || req.objects.is_empty() || req.objects.len() > MAX_MULTI_DELETE {
        return Err(RgwError::MalformedXml);
    }
    Ok(req)
}

/// `RGWDeleteMultiObj_ObjStore_S3::send_partial_response` / `end_response`.
#[derive(Debug, Serialize)]
#[serde(rename = "DeleteResult")]
pub(crate) struct DeleteResult {
    #[serde(rename = "@xmlns")]
    pub(crate) xmlns: &'static str,
    #[serde(rename = "Deleted")]
    pub(crate) deleted: Vec<DeletedXml>,
    #[serde(rename = "Error")]
    pub(crate) errors: Vec<DeleteErrorXml>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DeletedXml {
    #[serde(rename = "Key")]
    pub(crate) key: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct DeleteErrorXml {
    #[serde(rename = "Key")]
    pub(crate) key: String,
    #[serde(rename = "Code")]
    pub(crate) code: String,
    #[serde(rename = "Message")]
    pub(crate) message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use rgw_rest::iso8601_millis;

    const D: &str = XML_DECL;

    #[test]
    fn list_all_my_buckets() {
        let t = Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap() + chrono::Duration::milliseconds(67);
        let doc = ListAllMyBucketsResult {
            xmlns: XMLNS,
            owner: OwnerXml { id: "alice".into(), display_name: "Alice".into() },
            buckets: BucketsXml {
                bucket: vec![
                    BucketXml { name: "a".into(), creation_date: iso8601_millis(t) },
                    BucketXml { name: "b".into(), creation_date: iso8601_millis(t) },
                ],
            },
        };
        assert_eq!(
            to_xml(&doc).unwrap(),
            format!(
                "{D}<ListAllMyBucketsResult xmlns=\"{XMLNS}\"><Owner><ID>alice</ID><DisplayName>Alice</DisplayName></Owner>\
                 <Buckets><Bucket><Name>a</Name><CreationDate>2024-01-02T03:04:05.067Z</CreationDate></Bucket>\
                 <Bucket><Name>b</Name><CreationDate>2024-01-02T03:04:05.067Z</CreationDate></Bucket></Buckets>\
                 </ListAllMyBucketsResult>"
            )
        );
    }

    #[test]
    fn list_all_my_buckets_empty() {
        let doc = ListAllMyBucketsResult {
            xmlns: XMLNS,
            owner: OwnerXml { id: "a".into(), display_name: String::new() },
            buckets: BucketsXml { bucket: vec![] },
        };
        assert_eq!(
            to_xml(&doc).unwrap(),
            format!("{D}<ListAllMyBucketsResult xmlns=\"{XMLNS}\"><Owner><ID>a</ID><DisplayName/></Owner><Buckets/></ListAllMyBucketsResult>")
        );
    }

    fn contents(owner: bool) -> ContentsXml {
        ContentsXml {
            key: "dir/a&b".into(),
            last_modified: "2024-01-02T03:04:05.000Z".into(),
            etag: "\"0123abcd\"".into(),
            size: 42,
            storage_class: "STANDARD".into(),
            owner: owner.then(|| OwnerXml { id: "alice".into(), display_name: "Alice".into() }),
        }
    }

    #[test]
    fn list_v1_truncated() {
        let doc = ListBucketResultV1 {
            xmlns: XMLNS,
            name: "b".into(),
            prefix: String::new(),
            marker: String::new(),
            max_keys: 1000,
            delimiter: Some("/".into()),
            encoding_type: None,
            is_truncated: true,
            next_marker: Some("dir/a&b".into()),
            contents: vec![contents(true)],
            common_prefixes: vec![CommonPrefixXml { prefix: "p/".into() }],
        };
        assert_eq!(
            to_xml(&doc).unwrap(),
            format!(
                "{D}<ListBucketResult xmlns=\"{XMLNS}\"><Name>b</Name><Prefix/><Marker/><MaxKeys>1000</MaxKeys>\
                 <Delimiter>/</Delimiter><IsTruncated>true</IsTruncated><NextMarker>dir/a&amp;b</NextMarker>\
                 <Contents><Key>dir/a&amp;b</Key><LastModified>2024-01-02T03:04:05.000Z</LastModified>\
                 <ETag>\"0123abcd\"</ETag><Size>42</Size><StorageClass>STANDARD</StorageClass>\
                 <Owner><ID>alice</ID><DisplayName>Alice</DisplayName></Owner></Contents>\
                 <CommonPrefixes><Prefix>p/</Prefix></CommonPrefixes></ListBucketResult>"
            )
        );
    }

    #[test]
    fn list_v1_not_truncated_omits_next_marker() {
        let doc = ListBucketResultV1 {
            xmlns: XMLNS,
            name: "b".into(),
            prefix: "x".into(),
            marker: "m".into(),
            max_keys: 5,
            delimiter: None,
            encoding_type: Some("url"),
            is_truncated: false,
            next_marker: None,
            contents: vec![],
            common_prefixes: vec![],
        };
        assert_eq!(
            to_xml(&doc).unwrap(),
            format!(
                "{D}<ListBucketResult xmlns=\"{XMLNS}\"><Name>b</Name><Prefix>x</Prefix><Marker>m</Marker>\
                 <MaxKeys>5</MaxKeys><EncodingType>url</EncodingType><IsTruncated>false</IsTruncated></ListBucketResult>"
            )
        );
    }

    #[test]
    fn list_v2() {
        let doc = ListBucketResultV2 {
            xmlns: XMLNS,
            name: "b".into(),
            prefix: String::new(),
            start_after: None,
            continuation_token: Some("k0".into()),
            next_continuation_token: Some("k1".into()),
            key_count: 1,
            max_keys: 1,
            delimiter: None,
            encoding_type: None,
            is_truncated: true,
            contents: vec![contents(false)],
            common_prefixes: vec![],
        };
        assert_eq!(
            to_xml(&doc).unwrap(),
            format!(
                "{D}<ListBucketResult xmlns=\"{XMLNS}\"><Name>b</Name><Prefix/><ContinuationToken>k0</ContinuationToken>\
                 <NextContinuationToken>k1</NextContinuationToken><KeyCount>1</KeyCount><MaxKeys>1</MaxKeys>\
                 <IsTruncated>true</IsTruncated><Contents><Key>dir/a&amp;b</Key>\
                 <LastModified>2024-01-02T03:04:05.000Z</LastModified><ETag>\"0123abcd\"</ETag>\
                 <Size>42</Size><StorageClass>STANDARD</StorageClass></Contents></ListBucketResult>"
            )
        );
    }

    #[test]
    fn location_and_versioning() {
        let loc = LocationConstraint { xmlns: XMLNS, location: "default".into() };
        assert_eq!(
            to_xml(&loc).unwrap(),
            format!("{D}<LocationConstraint xmlns=\"{XMLNS}\">default</LocationConstraint>")
        );
        let ver = VersioningConfiguration { xmlns: XMLNS };
        assert_eq!(to_xml(&ver).unwrap(), format!("{D}<VersioningConfiguration xmlns=\"{XMLNS}\"/>"));
    }

    #[test]
    fn delete_request_parses() {
        let body = format!(
            "<?xml version=\"1.0\"?><Delete xmlns=\"{XMLNS}\"><Quiet>true</Quiet>\
             <Object><Key>a/b</Key></Object><Object><Key> sp </Key></Object><Object><Key>c &amp; d</Key><VersionId>null</VersionId></Object></Delete>"
        );
        let req = parse_delete_request(body.as_bytes()).unwrap();
        assert!(req.quiet);
        assert_eq!(
            req.objects,
            vec![
                DeleteObject { key: "a/b".into(), version_id: None },
                DeleteObject { key: " sp ".into(), version_id: None },
                DeleteObject { key: "c & d".into(), version_id: Some("null".into()) },
            ]
        );
        let loud = parse_delete_request(b"<Delete><Object><Key>k</Key></Object></Delete>").unwrap();
        assert!(!loud.quiet);
    }

    #[test]
    fn delete_request_rejects_bad_bodies() {
        for bad in ["", "<Delete></Delete>", "<Delete><Quiet>true</Quiet></Delete>", "<Delete><Object></Object></Delete>", "not xml <", "<Other><Object><Key>k</Key></Object></Other>", "<Delete><Object><Key>k</Object></Delete>"] {
            assert_eq!(parse_delete_request(bad.as_bytes()).err(), Some(RgwError::MalformedXml), "{bad:?}");
        }
        let many = format!("<Delete>{}</Delete>", "<Object><Key>k</Key></Object>".repeat(MAX_MULTI_DELETE + 1));
        assert_eq!(parse_delete_request(many.as_bytes()).err(), Some(RgwError::MalformedXml));
    }

    #[test]
    fn delete_result() {
        let doc = DeleteResult {
            xmlns: XMLNS,
            deleted: vec![DeletedXml { key: "a".into() }],
            errors: vec![DeleteErrorXml { key: "b".into(), code: "AccessDenied".into(), message: "access denied".into() }],
        };
        assert_eq!(
            to_xml(&doc).unwrap(),
            format!(
                "{D}<DeleteResult xmlns=\"{XMLNS}\"><Deleted><Key>a</Key></Deleted>\
                 <Error><Key>b</Key><Code>AccessDenied</Code><Message>access denied</Message></Error></DeleteResult>"
            )
        );
    }
}
