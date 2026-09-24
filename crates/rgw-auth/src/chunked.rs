//! `aws-chunked` payload decoding: the counterpart of `AWSv4ComplMulti`
//! (signed chunks) and the trailer handling in `rgw_auth_s3.cc` /
//! `rgw_cksum_pipe.cc`, over a fully buffered body.
//!
//! Wire format, per chunk: `<hex size>[;chunk-signature=<hex>]\r\n<data>\r\n`,
//! ending with a zero-size chunk. Without trailers the zero chunk is followed
//! by `\r\n`; with trailers, by `name:value\r\n` lines and a blank line.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rgw_types::{RgwError, RgwResult};

use crate::sigv4::{self, CredentialScope};

/// What each chunk's signature is checked against.
pub struct ChunkSigner<'a> {
    pub signing_key: &'a [u8; 32],
    pub amz_date: &'a str,
    pub scope: &'a CredentialScope,
    /// The request's `Authorization` signature, which seeds the chain.
    pub seed_signature: &'a str,
}

/// A decoded body and the trailer headers that followed it, names
/// lowercased.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DecodedChunked {
    pub payload: Vec<u8>,
    pub trailers: Vec<(String, String)>,
}

impl DecodedChunked {
    pub fn trailer(&self, name: &str) -> Option<&str> {
        self.trailers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

fn bad_body(why: &str) -> RgwError {
    RgwError::InvalidRequest(format!("malformed aws-chunked body: {why}"))
}

/// Split off one CRLF-terminated line starting at `*pos`.
fn next_line<'b>(body: &'b [u8], pos: &mut usize) -> Option<&'b [u8]> {
    let rest = body.get(*pos..)?;
    let end = rest.windows(2).position(|w| w == b"\r\n")?;
    *pos += end + 2;
    Some(&rest[..end])
}

/// Decode an `aws-chunked` body. With a `signer`, every chunk (including
/// the final empty one) must carry a `chunk-signature` that continues the
/// chain; without one, chunk extensions are ignored. `with_trailers`
/// selects the `*-TRAILER` framing after the zero chunk.
///
/// Trailer signatures (`x-amz-trailer-signature`) are returned as a trailer
/// but not verified.
pub fn decode_aws_chunked(
    body: &[u8],
    signer: Option<&ChunkSigner<'_>>,
    with_trailers: bool,
) -> RgwResult<DecodedChunked> {
    let mut out = DecodedChunked::default();
    let mut prev_signature = signer.map(|s| s.seed_signature.to_owned());
    let mut pos = 0;
    loop {
        let line = next_line(body, &mut pos).ok_or_else(|| bad_body("truncated chunk header"))?;
        let line = std::str::from_utf8(line).map_err(|_| bad_body("chunk header is not UTF-8"))?;
        let (size, ext) = match line.split_once(';') {
            Some((size, ext)) => (size, Some(ext)),
            None => (line, None),
        };
        let size = usize::from_str_radix(size.trim(), 16).map_err(|_| bad_body("bad chunk size"))?;
        let end = pos.checked_add(size).filter(|&e| e <= body.len()).ok_or_else(|| bad_body("truncated chunk"))?;
        let data = &body[pos..end];
        pos = end;

        if let (Some(signer), Some(prev)) = (signer, prev_signature.as_mut()) {
            let provided = ext
                .and_then(|e| e.trim().strip_prefix("chunk-signature="))
                .ok_or_else(|| bad_body("missing chunk-signature"))?;
            let sts = sigv4::chunk_string_to_sign(signer.amz_date, signer.scope, prev, data);
            let computed = sigv4::signature_bytes(signer.signing_key, &sts)?;
            if !sigv4::signature_matches(&computed, provided) {
                return Err(RgwError::SignatureDoesNotMatch);
            }
            *prev = provided.to_ascii_lowercase();
        }

        if size == 0 {
            break;
        }
        out.payload.extend_from_slice(data);
        if body.get(pos..pos + 2) != Some(b"\r\n".as_slice()) {
            return Err(bad_body("chunk data not followed by CRLF"));
        }
        pos += 2;
    }

    // After the zero chunk: optional trailer lines, then a blank line. A
    // body that simply ends here is accepted; some clients omit the CRLF.
    while let Some(line) = next_line(body, &mut pos) {
        if line.is_empty() {
            break;
        }
        if !with_trailers {
            return Err(bad_body("trailer in a payload that declared none"));
        }
        let line = std::str::from_utf8(line).map_err(|_| bad_body("trailer is not UTF-8"))?;
        let (name, value) = line.split_once(':').ok_or_else(|| bad_body("trailer without ':'"))?;
        out.trailers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    if pos < body.len() && body[pos..] != *b"\r\n" {
        return Err(bad_body("data after the final chunk"));
    }
    Ok(out)
}

/// Check the trailing `x-amz-checksum-crc32` against the decoded payload,
/// when the client sent one. Other checksum algorithms pass unchecked.
pub fn verify_trailer_checksum(decoded: &DecodedChunked) -> RgwResult<()> {
    let Some(want) = decoded.trailer("x-amz-checksum-crc32") else {
        return Ok(());
    };
    let want = BASE64.decode(want).map_err(|_| RgwError::InvalidDigest)?;
    if want != crc32fast::hash(&decoded.payload).to_be_bytes() {
        return Err(RgwError::BadDigest);
    }
    Ok(())
}

/// Build a signed `aws-chunked` body with the same primitives the
/// decoder checks against.
#[cfg(test)]
pub(crate) fn encode_signed(chunks: &[&[u8]], key: &[u8; 32], date: &str, scope: &CredentialScope, seed: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut prev = seed.to_owned();
    for chunk in chunks.iter().copied().chain(std::iter::once(&b""[..])) {
        prev = sigv4::signature(key, &sigv4::chunk_string_to_sign(date, scope, &prev, chunk)).unwrap();
        out.extend_from_slice(format!("{:x};chunk-signature={prev}\r\n", chunk.len()).as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> CredentialScope {
        CredentialScope { date: "20130524".into(), region: "us-east-1".into(), service: "s3".into() }
    }

    #[test]
    fn signed_chunks_decode_and_verify() {
        let key = [7u8; 32];
        let seed = "ab".repeat(32);
        let body = encode_signed(&[b"hello ", b"world"], &key, "20130524T000000Z", &scope(), &seed);
        let sc = scope();
        let signer = ChunkSigner { signing_key: &key, amz_date: "20130524T000000Z", scope: &sc, seed_signature: &seed };
        let d = decode_aws_chunked(&body, Some(&signer), false).unwrap();
        assert_eq!(d.payload, b"hello world");

        let wrong_seed = "cd".repeat(32);
        let signer = ChunkSigner { seed_signature: &wrong_seed, ..signer };
        assert_eq!(decode_aws_chunked(&body, Some(&signer), false), Err(RgwError::SignatureDoesNotMatch));
    }

    #[test]
    fn tampered_chunk_fails() {
        let key = [7u8; 32];
        let seed = "ab".repeat(32);
        let sc = scope();
        let mut body = encode_signed(&[b"hello"], &key, "20130524T000000Z", &sc, &seed);
        let at = body.windows(5).position(|w| w == b"hello").unwrap();
        body[at] = b'j';
        let signer = ChunkSigner { signing_key: &key, amz_date: "20130524T000000Z", scope: &sc, seed_signature: &seed };
        assert_eq!(decode_aws_chunked(&body, Some(&signer), false), Err(RgwError::SignatureDoesNotMatch));
    }

    #[test]
    fn unsigned_trailer_with_crc32() {
        let crc = BASE64.encode(crc32fast::hash(b"hello world").to_be_bytes());
        let body = format!("6\r\nhello \r\n5\r\nworld\r\n0\r\nx-amz-checksum-crc32:{crc}\r\n\r\n");
        let d = decode_aws_chunked(body.as_bytes(), None, true).unwrap();
        assert_eq!(d.payload, b"hello world");
        assert_eq!(d.trailer("x-amz-checksum-crc32"), Some(crc.as_str()));
        verify_trailer_checksum(&d).unwrap();

        let bad = BASE64.encode(0u32.to_be_bytes());
        let body = format!("5\r\nhello\r\n0\r\nx-amz-checksum-crc32: {bad}\r\n\r\n");
        let d = decode_aws_chunked(body.as_bytes(), None, true).unwrap();
        assert_eq!(verify_trailer_checksum(&d), Err(RgwError::BadDigest));
    }

    #[test]
    fn malformed_bodies_are_rejected() {
        for bad in [
            &b"5\r\nhel"[..],
            b"zz\r\nhello\r\n0\r\n\r\n",
            b"5\r\nhelloXX0\r\n\r\n",
            b"5\r\nhello\r\n",
            b"0\r\nx-amz-checksum-crc32:AAAAAA==\r\n\r\n",
            b"0\r\n\r\ngarbage",
        ] {
            let err = decode_aws_chunked(bad, None, false).unwrap_err();
            assert!(matches!(err, RgwError::InvalidRequest(_)), "{:?}: {err:?}", String::from_utf8_lossy(bad));
        }
    }
}
