//! HTTP `Range` handling: `RGWGetObj::parse_range` in `rgw_op.cc`.

use rgw_sal::ByteRange;

/// Parse `bytes=a-b`, `bytes=a-` or `bytes=-n`.
///
/// `None` for anything else, including multiple ranges and `a > b`: RFC
/// 9110 has a server ignore a Range it cannot parse, and S3 then serves the
/// whole object. Unsatisfiable ranges parse fine and fail later.
pub(crate) fn parse_range(header: &str) -> Option<ByteRange> {
    let spec = header.trim().strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = (start.trim(), end.trim());
    let num = |s: &str| -> Option<u64> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    if start.is_empty() {
        return Some(ByteRange::Suffix(num(end)?));
    }
    let start = num(start)?;
    if end.is_empty() {
        return Some(ByteRange::Absolute { start, end: None });
    }
    let end = num(end)?;
    (start <= end).then_some(ByteRange::Absolute { start, end: Some(end) })
}

/// Resolve a range against an object size under the SAL's `get_object`
/// contract; `None` is unsatisfiable. HEAD needs this because it never asks
/// the driver for data.
pub(crate) fn resolve_range(range: ByteRange, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let last = size - 1;
    match range {
        ByteRange::Absolute { start, end } => {
            (start <= last).then(|| (start, end.map_or(last, |e| e.min(last))))
        }
        ByteRange::Suffix(0) => None,
        ByteRange::Suffix(n) => Some((size.saturating_sub(n), last)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_forms() {
        assert_eq!(parse_range("bytes=0-9"), Some(ByteRange::Absolute { start: 0, end: Some(9) }));
        assert_eq!(parse_range("bytes=5-"), Some(ByteRange::Absolute { start: 5, end: None }));
        assert_eq!(parse_range("bytes=-3"), Some(ByteRange::Suffix(3)));
        assert_eq!(parse_range(" bytes= 1 - 2 "), Some(ByteRange::Absolute { start: 1, end: Some(2) }));
    }

    #[test]
    fn malformed_is_ignored() {
        for bad in ["", "bytes=", "bytes=-", "bytes=a-b", "items=0-1", "bytes=0-1,3-4", "bytes=5-2", "bytes=+1-2", "bytes=1"] {
            assert_eq!(parse_range(bad), None, "{bad}");
        }
    }

    #[test]
    fn unsatisfiable_still_parses() {
        assert_eq!(parse_range("bytes=100-"), Some(ByteRange::Absolute { start: 100, end: None }));
        assert_eq!(parse_range("bytes=-0"), Some(ByteRange::Suffix(0)));
    }

    #[test]
    fn resolves_per_sal_contract() {
        let abs = |start, end| ByteRange::Absolute { start, end };
        assert_eq!(resolve_range(abs(0, Some(4)), 10), Some((0, 4)));
        assert_eq!(resolve_range(abs(3, None), 10), Some((3, 9)));
        assert_eq!(resolve_range(abs(3, Some(99)), 10), Some((3, 9)));
        assert_eq!(resolve_range(abs(10, None), 10), None);
        assert_eq!(resolve_range(ByteRange::Suffix(4), 10), Some((6, 9)));
        assert_eq!(resolve_range(ByteRange::Suffix(40), 10), Some((0, 9)));
        assert_eq!(resolve_range(ByteRange::Suffix(0), 10), None);
        assert_eq!(resolve_range(abs(0, None), 0), None);
    }
}
