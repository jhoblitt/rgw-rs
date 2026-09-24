//! The query string: `RGWHTTPArgs` from `rgw_common.h`, which RGW parses
//! once into `req_info::args` and consults for both parameters and
//! subresources (`?acl`, `?location`, ...).

use percent_encoding::percent_decode_str;

/// Bucket subresources this spike does not serve. Each maps to its own
/// `RGWOp` in RGW (`RGWGetACLs`, `RGWGetBucketPolicy`, `RGWGetLC`, ...).
pub(crate) const BUCKET_UNSUPPORTED: &[&str] = &[
    "acl",
    "policy",
    "lifecycle",
    "cors",
    "tagging",
    "uploads",
    "versions",
    "website",
    "logging",
    "notification",
    "encryption",
    "object-lock",
    "replication",
    "requestPayment",
    "publicAccessBlock",
    "ownershipControls",
    "intelligent-tiering",
    "inventory",
    "metrics",
    "analytics",
    "accelerate",
    "policyStatus",
];

/// Object subresources this spike does not serve (multipart, ACLs, tags,
/// object lock, torrents, `GetObjectAttributes`, S3 Select).
pub(crate) const OBJECT_UNSUPPORTED: &[&str] = &[
    "acl",
    "tagging",
    "uploads",
    "uploadId",
    "partNumber",
    "legal-hold",
    "retention",
    "torrent",
    "attributes",
    "select",
    "restore",
];

/// Parsed query parameters in request order. A bare `?acl` is present with
/// an empty value, as `RGWHTTPArgs::exists` sees it.
#[derive(Debug, Default)]
pub(crate) struct Args(Vec<(String, String)>);

impl Args {
    pub(crate) fn parse(query: Option<&str>) -> Self {
        let Some(query) = query else { return Self::default() };
        let pairs = query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                (decode(k), decode(v))
            })
            .collect();
        Self(pairs)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub(crate) fn exists(&self, name: &str) -> bool {
        self.0.iter().any(|(k, _)| k == name)
    }

    /// The first of `names` present, for rejecting unsupported subresources.
    pub(crate) fn any_of(&self, names: &[&'static str]) -> Option<&'static str> {
        names.iter().copied().find(|n| self.exists(n))
    }
}

/// `application/x-www-form-urlencoded` decoding: `+` is a space.
fn decode(s: &str) -> String {
    let s = s.replace('+', " ");
    percent_decode_str(&s).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_valued_params() {
        let a = Args::parse(Some("acl&prefix=a%2Fb+c&max-keys=5&empty="));
        assert!(a.exists("acl"));
        assert_eq!(a.get("acl"), Some(""));
        assert_eq!(a.get("prefix"), Some("a/b c"));
        assert_eq!(a.get("max-keys"), Some("5"));
        assert_eq!(a.get("empty"), Some(""));
        assert_eq!(a.get("nope"), None);
        assert_eq!(a.any_of(BUCKET_UNSUPPORTED), Some("acl"));
        assert_eq!(Args::parse(None).any_of(OBJECT_UNSUPPORTED), None);
    }
}
