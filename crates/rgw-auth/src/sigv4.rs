//! AWS Signature Version 4 primitives: parsing the `Authorization` header,
//! building the canonical request and string to sign, and deriving the
//! signing key. The counterparts of `parse_v4_auth_header`,
//! `get_v4_canonical_*`, `get_v4_string_to_sign`, `get_v4_signing_key` and
//! `get_v4_signature` in `rgw_auth_s3.{h,cc}`.
//!
//! Everything here is a pure function of its inputs so it can be checked
//! against the AWS published examples.

use std::fmt::{self, Write as _};

use axum::http::HeaderMap;
use chrono::{DateTime, NaiveDateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use percent_encoding::percent_decode_str;
use rgw_types::{RgwError, RgwResult};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// `AWS4_HMAC_SHA256_STR`.
pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";
/// `AWS4_HMAC_SHA256_PAYLOAD_STR`: the algorithm line of a chunk's string
/// to sign.
pub const CHUNK_ALGORITHM: &str = "AWS4-HMAC-SHA256-PAYLOAD";
/// `AWS4_EMPTY_PAYLOAD_HASH`.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// `AWS4_UNSIGNED_PAYLOAD_HASH`.
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
/// `AWS4_HMAC_SHA256_PAYLOAD_HASH`.
pub const STREAMING_SIGNED: &str = "STREAMING-AWS4-HMAC-SHA256-PAYLOAD";
/// `AWS4_HMAC_SHA256_PAYLOAD_TRAILER_HASH`.
pub const STREAMING_SIGNED_TRAILER: &str = "STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER";
/// `AWS4_UNSIGNED_PAYLOAD_TRAILER_HASH`.
pub const STREAMING_UNSIGNED_TRAILER: &str = "STREAMING-UNSIGNED-PAYLOAD-TRAILER";

const AMZ_DATE_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// The credential scope, `<date>/<region>/<service>/aws4_request`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialScope {
    /// `yyyymmdd`.
    pub date: String,
    pub region: String,
    pub service: String,
}

impl fmt::Display for CredentialScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}/aws4_request", self.date, self.region, self.service)
    }
}

/// A parsed `Authorization: AWS4-HMAC-SHA256 ...` header, what RGW's
/// `parse_v4_auth_header` splits out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationV4 {
    pub access_key_id: String,
    pub scope: CredentialScope,
    /// Lowercase, strictly ascending, and containing `host`.
    pub signed_headers: Vec<String>,
    /// Lowercase hex, 64 characters.
    pub signature: String,
}

fn malformed(why: &str) -> RgwError {
    RgwError::AuthorizationHeaderMalformed(why.to_owned())
}

/// `parse_v4_auth_header`: split the header value into its three fields.
/// The caller has already established that it starts with [`ALGORITHM`].
pub fn parse_authorization(value: &str) -> RgwResult<AuthorizationV4> {
    let rest = value
        .strip_prefix(ALGORITHM)
        .filter(|r| r.starts_with([' ', '\t']))
        .ok_or_else(|| malformed("unsupported authorization algorithm"))?;

    let (mut credential, mut signed_headers, mut signature) = (None, None, None);
    for field in rest.split(',').map(str::trim).filter(|f| !f.is_empty()) {
        let (name, val) = field.split_once('=').ok_or_else(|| malformed("field without '='"))?;
        let slot = match name.trim() {
            "Credential" => &mut credential,
            "SignedHeaders" => &mut signed_headers,
            "Signature" => &mut signature,
            _ => return Err(malformed("unknown authorization field")),
        };
        if slot.replace(val.trim()).is_some() {
            return Err(malformed("duplicate authorization field"));
        }
    }
    let credential = credential.ok_or_else(|| malformed("missing Credential"))?;
    let signed_headers = signed_headers.ok_or_else(|| malformed("missing SignedHeaders"))?;
    let signature = signature.ok_or_else(|| malformed("missing Signature"))?;

    // The access key id is everything before the last four components.
    let mut parts = credential.rsplitn(5, '/');
    let terminator = parts.next().unwrap_or_default();
    let service = parts.next().ok_or_else(|| malformed("short Credential"))?;
    let region = parts.next().ok_or_else(|| malformed("short Credential"))?;
    let date = parts.next().ok_or_else(|| malformed("short Credential"))?;
    let access_key_id = parts.next().ok_or_else(|| malformed("short Credential"))?;
    if access_key_id.is_empty() {
        return Err(malformed("empty access key id"));
    }
    if terminator != "aws4_request" {
        return Err(malformed("credential scope must end in aws4_request"));
    }
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed("credential date must be yyyymmdd"));
    }
    if service != "s3" {
        return Err(malformed("credential service must be s3"));
    }

    let signed_headers: Vec<String> = signed_headers.split(';').map(str::to_owned).collect();
    if signed_headers.iter().any(|h| h.is_empty() || h.bytes().any(|b| b.is_ascii_uppercase())) {
        return Err(malformed("SignedHeaders must be non-empty lowercase names"));
    }
    if !signed_headers.windows(2).all(|w| w[0] < w[1]) {
        return Err(malformed("SignedHeaders must be sorted"));
    }
    if !signed_headers.iter().any(|h| h == "host") {
        return Err(malformed("SignedHeaders must include host"));
    }

    if signature.len() != 64 || !signature.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(malformed("Signature must be 64 hex characters"));
    }

    Ok(AuthorizationV4 {
        access_key_id: access_key_id.to_owned(),
        scope: CredentialScope {
            date: date.to_owned(),
            region: region.to_owned(),
            service: service.to_owned(),
        },
        signed_headers,
        signature: signature.to_ascii_lowercase(),
    })
}

/// Parse `x-amz-date` (ISO 8601 basic, `20130524T000000Z`).
pub fn parse_amz_date(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, AMZ_DATE_FORMAT).ok().map(|t| t.and_utc())
}

/// Format a time as `x-amz-date` does; the form the string to sign carries
/// even when the request only had a `Date` header.
pub fn format_amz_date(t: DateTime<Utc>) -> String {
    t.format(AMZ_DATE_FORMAT).to_string()
}

/// Parse an RFC 7231 IMF-fixdate `Date` header. The obsolete RFC 850 and
/// asctime forms are not accepted.
pub fn parse_http_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(s).ok().map(|t| t.with_timezone(&Utc))
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

/// `aws4_uri_encode`.
fn uri_encode(out: &mut String, bytes: &[u8], encode_slash: bool) {
    for &b in bytes {
        if is_unreserved(b) || (b == b'/' && !encode_slash) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

/// `aws4_uri_recode`: decode, then re-encode with the AWS unreserved set.
fn uri_recode(s: &str, encode_slash: bool) -> String {
    let decoded: Vec<u8> = percent_decode_str(s).collect();
    let mut out = String::with_capacity(decoded.len());
    uri_encode(&mut out, &decoded, encode_slash);
    out
}

/// `get_v4_canonical_uri`. Dot segments are left alone: S3 object names may
/// contain them.
pub fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_owned();
    }
    uri_recode(path, false)
}

/// `get_v4_canonical_qs`. `+` is read as an encoded space, as RGW does,
/// so a form-encoded query and a percent-encoded one sign the same.
pub fn canonical_query(query: &str) -> String {
    let query = query.replace('+', "%20");
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (name, value) = s.split_once('=').unwrap_or((s, ""));
            (uri_recode(name, true), uri_recode(value, true))
        })
        .collect();
    pairs.sort();
    let mut out = String::new();
    for (i, (name, value)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(name);
        out.push('=');
        out.push_str(value);
    }
    out
}

/// `get_v4_canonical_headers`: one `name:value\n` line per signed header.
///
/// `authority` stands in for `host` when there is no `Host` header, which
/// is how an HTTP/2 request carries it. A signed header the request lacks
/// is skipped, as RGW does; the signature then fails to match.
pub fn canonical_headers(headers: &HeaderMap, authority: Option<&str>, signed_headers: &[String]) -> String {
    let mut out = String::new();
    for name in signed_headers {
        let mut values: Vec<String> = headers
            .get_all(name.as_str())
            .iter()
            .map(|v| collapse_whitespace(&String::from_utf8_lossy(v.as_bytes())))
            .collect();
        if values.is_empty() && name == "host" {
            values.extend(authority.map(collapse_whitespace));
        }
        if values.is_empty() {
            continue;
        }
        out.push_str(name);
        out.push(':');
        out.push_str(&values.join(","));
        out.push('\n');
    }
    out
}

fn collapse_whitespace(s: &str) -> String {
    s.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

/// `get_v4_canonical_request_hash` before the hashing: the six-part
/// canonical request.
pub fn canonical_request(
    method: &str,
    path: &str,
    query: &str,
    headers: &HeaderMap,
    authority: Option<&str>,
    signed_headers: &[String],
    payload_hash: &str,
) -> String {
    format!(
        "{method}\n{}\n{}\n{}\n{}\n{payload_hash}",
        canonical_uri(path),
        canonical_query(query),
        canonical_headers(headers, authority, signed_headers),
        signed_headers.join(";"),
    )
}

/// Lowercase hex SHA-256, the form `x-amz-content-sha256` and the string
/// to sign carry.
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// `get_v4_string_to_sign`.
pub fn string_to_sign(amz_date: &str, scope: &CredentialScope, canonical_request: &str) -> String {
    format!("{ALGORITHM}\n{amz_date}\n{scope}\n{}", sha256_hex(canonical_request.as_bytes()))
}

/// The string to sign for one `aws-chunked` chunk
/// (`AWSv4ComplMulti::calc_chunk_signature`).
pub fn chunk_string_to_sign(amz_date: &str, scope: &CredentialScope, prev_signature: &str, chunk: &[u8]) -> String {
    format!(
        "{CHUNK_ALGORITHM}\n{amz_date}\n{scope}\n{prev_signature}\n{EMPTY_SHA256}\n{}",
        sha256_hex(chunk)
    )
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> RgwResult<[u8; 32]> {
    let mut mac = HmacSha256::new_from_slice(key).map_err(RgwError::internal)?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().into())
}

/// `get_v4_signing_key`.
pub fn signing_key(secret: &str, scope: &CredentialScope) -> RgwResult<[u8; 32]> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), scope.date.as_bytes())?;
    let k_region = hmac_sha256(&k_date, scope.region.as_bytes())?;
    let k_service = hmac_sha256(&k_region, scope.service.as_bytes())?;
    hmac_sha256(&k_service, b"aws4_request")
}

/// `get_v4_signature`, as raw bytes.
pub fn signature_bytes(signing_key: &[u8; 32], string_to_sign: &str) -> RgwResult<[u8; 32]> {
    hmac_sha256(signing_key, string_to_sign.as_bytes())
}

/// `get_v4_signature`, as the lowercase hex the headers carry.
pub fn signature(signing_key: &[u8; 32], string_to_sign: &str) -> RgwResult<String> {
    signature_bytes(signing_key, string_to_sign).map(hex::encode)
}

/// Compare a computed signature with a client-supplied hex string without
/// an early exit on the first differing byte.
pub fn signature_matches(computed: &[u8; 32], provided_hex: &str) -> bool {
    let mut provided = [0u8; 32];
    if hex::decode_to_slice(provided_hex, &mut provided).is_err() {
        return false;
    }
    computed.iter().zip(provided.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    const DATE: &str = "20130524T000000Z";
    const HOST: &str = "examplebucket.s3.amazonaws.com";

    fn scope() -> CredentialScope {
        CredentialScope { date: "20130524".into(), region: "us-east-1".into(), service: "s3".into() }
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("host", HeaderValue::from_static(HOST));
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_static(v));
        }
        h
    }

    fn signed(names: &str) -> Vec<String> {
        names.split(';').map(str::to_owned).collect()
    }

    fn sign(method: &str, path: &str, query: &str, h: &HeaderMap, names: &str, payload: &str) -> String {
        let creq = canonical_request(method, path, query, h, None, &signed(names), payload);
        let key = signing_key(SECRET, &scope()).unwrap();
        signature(&key, &string_to_sign(DATE, &scope(), &creq)).unwrap()
    }

    #[test]
    fn aws_example_get_object() {
        let h = headers(&[("range", "bytes=0-9"), ("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", DATE)]);
        let names = "host;range;x-amz-content-sha256;x-amz-date";
        assert_eq!(
            canonical_request("GET", "/test.txt", "", &h, None, &signed(names), EMPTY_SHA256),
            "GET\n/test.txt\n\nhost:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\n\
             x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             x-amz-date:20130524T000000Z\n\nhost;range;x-amz-content-sha256;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sign("GET", "/test.txt", "", &h, names, EMPTY_SHA256),
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn aws_example_put_object() {
        let payload = "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072";
        assert_eq!(sha256_hex(b"Welcome to Amazon S3."), payload);
        let h = headers(&[
            ("date", "Fri, 24 May 2013 00:00:00 GMT"),
            ("x-amz-date", DATE),
            ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ("x-amz-content-sha256", payload),
        ]);
        assert_eq!(canonical_uri("/test$file.text"), "/test%24file.text");
        assert_eq!(
            sign("PUT", "/test$file.text", "", &h, "date;host;x-amz-content-sha256;x-amz-date;x-amz-storage-class", payload),
            "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        );
    }

    #[test]
    fn aws_example_get_lifecycle() {
        let h = headers(&[("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", DATE)]);
        assert_eq!(canonical_query("lifecycle"), "lifecycle=");
        assert_eq!(
            sign("GET", "/", "lifecycle", &h, "host;x-amz-content-sha256;x-amz-date", EMPTY_SHA256),
            "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        );
    }

    /// The expected value as handed to this spike ended in
    /// `...bc5e8d3c8ffb96f4c3a7a3fb9`; it agreed with the computed one for 42
    /// of 64 hex digits, which no HMAC bug produces, so the tail was a
    /// transcription error. The value below is the one the AWS page shows.
    #[test]
    fn aws_example_list_objects() {
        let h = headers(&[("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", DATE)]);
        assert_eq!(
            sign("GET", "/", "max-keys=2&prefix=J", &h, "host;x-amz-content-sha256;x-amz-date", EMPTY_SHA256),
            "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        );
    }

    /// The seed and chunk signatures of the AWS "Signature Calculations for
    /// the Authorization Header: Transferring Payload in Multiple Chunks"
    /// example (66560 bytes of `a`, 64 KiB chunks).
    #[test]
    fn aws_example_chunked_upload() {
        let mut h = headers(&[
            ("content-encoding", "aws-chunked"),
            ("content-length", "66824"),
            ("x-amz-content-sha256", STREAMING_SIGNED),
            ("x-amz-date", DATE),
            ("x-amz-decoded-content-length", "66560"),
            ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
        ]);
        h.insert("host", HeaderValue::from_static("s3.amazonaws.com"));
        let names = "content-encoding;content-length;host;x-amz-content-sha256;x-amz-date;\
                     x-amz-decoded-content-length;x-amz-storage-class";
        let seed = sign("PUT", "/examplebucket/chunkObject.txt", "", &h, names, STREAMING_SIGNED);
        assert_eq!(seed, "4f232c4386841ef735655705268965c44a0e4690baa4adea153f7db9fa80a0a9");

        let key = signing_key(SECRET, &scope()).unwrap();
        let mut prev = seed;
        for (chunk, want) in [
            (vec![b'a'; 65536], "ad80c730a21e5b8d04586a2213dd63b9a0e99e0e2307b0ade35a65485a288648"),
            (vec![b'a'; 1024], "0055627c9e194cb4542bae2aa5492e3c1575bbb81b612b7d234b86a503ef5497"),
            (Vec::new(), "b6c6ea8a5354eaf15b3cb7646744f4275b71ea724fed81ceb9323e279d449df9"),
        ] {
            prev = signature(&key, &chunk_string_to_sign(DATE, &scope(), &prev, &chunk)).unwrap();
            assert_eq!(prev, want);
        }
    }

    #[test]
    fn uri_recoding_normalizes_client_encodings() {
        assert_eq!(canonical_uri(""), "/");
        assert_eq!(canonical_uri("/b/a%20b+c"), "/b/a%20b%2Bc");
        assert_eq!(canonical_uri("/b/a b"), "/b/a%20b");
        assert_eq!(canonical_uri("/b/%7euser/./x/../y"), "/b/~user/./x/../y");
        assert_eq!(canonical_uri("/b/%E2%82%AC"), "/b/%E2%82%AC");
        assert_eq!(canonical_uri("/b/a%2fb"), "/b/a/b");
    }

    #[test]
    fn query_is_recoded_and_sorted() {
        assert_eq!(canonical_query(""), "");
        assert_eq!(canonical_query("prefix=a/b&delimiter=%2F"), "delimiter=%2F&prefix=a%2Fb");
        assert_eq!(canonical_query("b=2&a=2&a=1&uploads"), "a=1&a=2&b=2&uploads=");
        assert_eq!(canonical_query("k=a+b&k2=%7e"), "k=a%20b&k2=~");
    }

    #[test]
    fn header_values_are_trimmed_collapsed_and_joined() {
        let mut h = HeaderMap::new();
        h.append("x-amz-meta-a", HeaderValue::from_static("  one   two "));
        h.append("x-amz-meta-a", HeaderValue::from_static("three"));
        let names = signed("host;x-amz-meta-a");
        assert_eq!(canonical_headers(&h, Some("h:8000"), &names), "host:h:8000\nx-amz-meta-a:one two,three\n");
    }

    #[test]
    fn authorization_parses() {
        let a = parse_authorization(
            "AWS4-HMAC-SHA256 Credential=AKID/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-date,Signature=ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .unwrap();
        assert_eq!(a.access_key_id, "AKID");
        assert_eq!(a.scope.to_string(), "20130524/us-east-1/s3/aws4_request");
        assert_eq!(a.signed_headers, ["host", "x-amz-date"]);
        assert!(a.signature.starts_with("abcdef"));
    }

    #[test]
    fn authorization_rejects_malformed() {
        let sig = "0".repeat(64);
        for bad in [
            format!("AWS4-HMAC-SHA256Credential=A/20130524/r/s3/aws4_request,SignedHeaders=host,Signature={sig}"),
            format!("AWS4-HMAC-SHA256 Credential=A/20130524/r/s3,SignedHeaders=host,Signature={sig}"),
            format!("AWS4-HMAC-SHA256 Credential=A/2013052/r/s3/aws4_request,SignedHeaders=host,Signature={sig}"),
            format!("AWS4-HMAC-SHA256 Credential=A/20130524/r/sqs/aws4_request,SignedHeaders=host,Signature={sig}"),
            format!("AWS4-HMAC-SHA256 Credential=A/20130524/r/s3/aws4_request,SignedHeaders=x-amz-date;host,Signature={sig}"),
            format!("AWS4-HMAC-SHA256 Credential=A/20130524/r/s3/aws4_request,SignedHeaders=x-amz-date,Signature={sig}"),
            "AWS4-HMAC-SHA256 Credential=A/20130524/r/s3/aws4_request,SignedHeaders=host,Signature=zz".to_owned(),
            "AWS4-HMAC-SHA256 Credential=A/20130524/r/s3/aws4_request,SignedHeaders=host".to_owned(),
        ] {
            let err = parse_authorization(&bad).unwrap_err();
            assert!(matches!(err, RgwError::AuthorizationHeaderMalformed(_)), "{bad}: {err:?}");
        }
    }

    #[test]
    fn dates_parse() {
        let t = parse_amz_date(DATE).unwrap();
        assert_eq!(parse_http_date("Fri, 24 May 2013 00:00:00 GMT"), Some(t));
        assert_eq!(format_amz_date(t), DATE);
        assert!(parse_amz_date("2013-05-24T00:00:00Z").is_none());
    }

    #[test]
    fn signature_comparison() {
        let key = signing_key(SECRET, &scope()).unwrap();
        let sig = signature_bytes(&key, "x").unwrap();
        assert!(signature_matches(&sig, &hex::encode(sig)));
        assert!(signature_matches(&sig, &hex::encode_upper(sig)));
        let mut other = sig;
        other[31] ^= 1;
        assert!(!signature_matches(&sig, &hex::encode(other)));
        assert!(!signature_matches(&sig, "00"));
    }
}
