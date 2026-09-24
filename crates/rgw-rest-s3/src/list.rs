//! Bucket listing: `RGWListBucket` in `rgw_op.cc` with
//! `RGWListBucket_ObjStore_S3` (V1) and `RGWListBucket_ObjStore_S3v2` (V2)
//! from `rgw_rest_s3.cc`.

use axum::response::Response;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use rgw_auth::Identity;
use rgw_rest::iso8601_millis;
use rgw_sal::{Driver, ListParams, ListResult};
use rgw_types::{ObjectInfo, RgwError, RgwResult};

use crate::args::Args;
use crate::error::OpResult;
use crate::perm::load_bucket_for;
use crate::xml::{
    CommonPrefixXml, ContentsXml, ListBucketResultV1, ListBucketResultV2, OwnerXml, XMLNS, xml_response,
};

/// S3's page ceiling and default for `max-keys`.
pub(crate) const MAX_KEYS: usize = 1000;

/// Attributes key RGW keeps the storage class under
/// (`RGW_ATTR_STORAGE_CLASS`), used because `Driver::put_object` has no
/// storage-class argument.
pub(crate) const RGW_ATTR_STORAGE_CLASS: &str = "user.rgw.storage_class";

/// `rgw_placement_rule::get_canonical_storage_class`: empty is `STANDARD`.
pub(crate) fn storage_class(info: &ObjectInfo) -> String {
    if !info.storage_class.is_empty() {
        return info.storage_class.clone();
    }
    info.attrs
        .get(RGW_ATTR_STORAGE_CLASS)
        .and_then(|v| std::str::from_utf8(v).ok())
        .filter(|s| !s.is_empty())
        .unwrap_or("STANDARD")
        .to_owned()
}

/// Everything `char_needs_url_encoding` in `rgw_common.cc` flags, except
/// `/`, which [`url_encode`] handles per call.
const URL_ENCODE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b',')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'>')
    .add(b'=')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b']')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'}');
const URL_ENCODE_SLASH: &AsciiSet = &URL_ENCODE.add(b'/');

/// `url_encode(src, dst, encode_slash)` in `rgw_common.cc`, as
/// `dump_urlsafe` applies it under `encoding-type=url`.
fn url_encode(s: &str, encode_slash: bool) -> String {
    if encode_slash {
        utf8_percent_encode(s, URL_ENCODE_SLASH).to_string()
    } else {
        utf8_percent_encode(s, URL_ENCODE).to_string()
    }
}

/// The parameters V1 and V2 share (`get_common_params`).
struct Common {
    prefix: String,
    delimiter: String,
    max_keys: usize,
    encode: bool,
}

impl Common {
    fn parse(args: &Args) -> RgwResult<Self> {
        Ok(Self {
            prefix: args.get("prefix").unwrap_or_default().to_owned(),
            delimiter: args.get("delimiter").unwrap_or_default().to_owned(),
            max_keys: parse_max_keys(args.get("max-keys"))?,
            // Any other value is ignored, as `strcasecmp(encoding_type, "url")`
            // in `send_response` has it.
            encode: args.get("encoding-type").is_some_and(|e| e.eq_ignore_ascii_case("url")),
        })
    }

    /// `dump_urlsafe` for a key (slash encoded) or prefix (slash kept).
    fn out(&self, s: &str, encode_slash: bool) -> String {
        if self.encode { url_encode(s, encode_slash) } else { s.to_owned() }
    }

    fn delimiter_out(&self) -> Option<String> {
        (!self.delimiter.is_empty()).then(|| self.out(&self.delimiter, false))
    }

    fn encoding_type(&self) -> Option<&'static str> {
        self.encode.then_some("url")
    }

    async fn list(&self, driver: &dyn Driver, bucket: &rgw_types::BucketKey, marker: &str) -> RgwResult<ListResult> {
        // `ListParams::max_keys == 0` means the driver default, not an empty
        // page, so `max-keys=0` never reaches the driver.
        if self.max_keys == 0 {
            return Ok(ListResult::default());
        }
        let params = ListParams {
            prefix: self.prefix.clone(),
            delimiter: self.delimiter.clone(),
            marker: marker.to_owned(),
            max_keys: self.max_keys,
        };
        driver.list_objects(bucket, &params).await
    }

    fn contents(&self, objects: Vec<ObjectInfo>, with_owner: bool) -> Vec<ContentsXml> {
        objects
            .into_iter()
            .map(|o| ContentsXml {
                key: self.out(&o.key.name, true),
                last_modified: iso8601_millis(o.mtime),
                etag: format!("\"{}\"", o.etag),
                size: o.size,
                storage_class: storage_class(&o),
                owner: with_owner
                    .then(|| OwnerXml { id: o.owner.id.to_string(), display_name: o.owner.display_name }),
            })
            .collect()
    }

    fn common_prefixes(&self, prefixes: Vec<String>) -> Vec<CommonPrefixXml> {
        prefixes.into_iter().map(|p| CommonPrefixXml { prefix: self.out(&p, false) }).collect()
    }
}

/// `RGWListBucket::parse_max_keys`: non-numeric or negative is
/// `InvalidArgument`; values above the ceiling are clamped.
fn parse_max_keys(v: Option<&str>) -> RgwResult<usize> {
    let Some(v) = v else { return Ok(MAX_KEYS) };
    let n: i64 = v
        .trim()
        .parse()
        .map_err(|_| RgwError::InvalidArgument(format!("invalid max-keys: {v}")))?;
    if n < 0 {
        return Err(RgwError::InvalidArgument(format!("invalid max-keys: {v}")));
    }
    Ok((n as u64).min(MAX_KEYS as u64) as usize)
}

/// The driver's resume point, falling back to the last entry when a driver
/// leaves `next_marker` empty on a truncated page.
fn next_marker(res: &ListResult) -> Option<String> {
    if !res.is_truncated {
        return None;
    }
    if !res.next_marker.is_empty() {
        return Some(res.next_marker.clone());
    }
    let last_key = res.objects.last().map(|o| o.key.name.as_str());
    let last_prefix = res.common_prefixes.last().map(String::as_str);
    last_key.max(last_prefix).map(str::to_owned)
}

/// `RGWListBucket` V1 (`GET /bucket`).
pub(crate) async fn list_objects_v1(
    driver: &dyn Driver,
    identity: &Identity,
    name: &str,
    args: &Args,
) -> OpResult<Response> {
    let common = Common::parse(args)?;
    let marker = args.get("marker").unwrap_or_default().to_owned();
    let bucket = load_bucket_for(driver, identity, name).await?;
    let res = common.list(driver, &bucket.bucket, &marker).await?;
    let next = next_marker(&res);
    let doc = ListBucketResultV1 {
        xmlns: XMLNS,
        name: bucket.bucket.name,
        prefix: common.out(&common.prefix, false),
        marker: common.out(&marker, true),
        max_keys: common.max_keys,
        delimiter: common.delimiter_out(),
        encoding_type: common.encoding_type(),
        is_truncated: res.is_truncated,
        next_marker: next.map(|m| common.out(&m, true)),
        contents: common.contents(res.objects, true),
        common_prefixes: common.common_prefixes(res.common_prefixes),
    };
    Ok(xml_response(&doc)?)
}

/// `RGWListBucket` V2 (`GET /bucket?list-type=2`). The continuation token
/// is the last key itself, as RGW's `next_marker` is; it takes precedence
/// over `start-after`.
pub(crate) async fn list_objects_v2(
    driver: &dyn Driver,
    identity: &Identity,
    name: &str,
    args: &Args,
) -> OpResult<Response> {
    let common = Common::parse(args)?;
    let token = args.get("continuation-token").map(str::to_owned);
    let start_after = args.get("start-after").map(str::to_owned);
    let fetch_owner = args.get("fetch-owner").is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let marker = token.clone().or_else(|| start_after.clone()).unwrap_or_default();

    let bucket = load_bucket_for(driver, identity, name).await?;
    let res = common.list(driver, &bucket.bucket, &marker).await?;
    let next = next_marker(&res);
    let doc = ListBucketResultV2 {
        xmlns: XMLNS,
        name: bucket.bucket.name,
        prefix: common.out(&common.prefix, false),
        start_after: start_after.map(|s| common.out(&s, true)),
        continuation_token: token,
        next_continuation_token: next,
        key_count: res.objects.len() + res.common_prefixes.len(),
        max_keys: common.max_keys,
        delimiter: common.delimiter_out(),
        encoding_type: common.encoding_type(),
        is_truncated: res.is_truncated,
        contents: common.contents(res.objects, fetch_owner),
        common_prefixes: common.common_prefixes(res.common_prefixes),
    };
    Ok(xml_response(&doc)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_keys() {
        assert_eq!(parse_max_keys(None), Ok(1000));
        assert_eq!(parse_max_keys(Some("5")), Ok(5));
        assert_eq!(parse_max_keys(Some("0")), Ok(0));
        assert_eq!(parse_max_keys(Some("5000")), Ok(1000));
        assert!(matches!(parse_max_keys(Some("-1")), Err(RgwError::InvalidArgument(_))));
        assert!(matches!(parse_max_keys(Some("x")), Err(RgwError::InvalidArgument(_))));
    }

    #[test]
    fn url_encoding_matches_rgw() {
        assert_eq!(url_encode("a/b c+d&e~f!g*(h)", true), "a%2Fb%20c%2Bd%26e~f!g*(h)");
        assert_eq!(url_encode("a/b c", false), "a/b%20c");
        assert_eq!(url_encode("é", false), "%C3%A9");
    }
}
