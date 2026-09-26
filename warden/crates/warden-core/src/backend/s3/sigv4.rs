//! AWS Signature Version 4, the ~150 lines that matter. Scoped to what the
//! S3 backend sends: path-style requests, a small set of signed headers, a
//! body we always hold in memory. Proven against AWS's published example
//! vector below — exact signature or bust.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type HmacSha256 = Hmac<Sha256>;

pub struct SigningKey<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub region: &'a str,
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

/// URI-encode per AWS rules: unreserved chars pass; '/' passes only when
/// `keep_slash` (canonical URIs keep it, query values do not).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Compute the Authorization header value. `headers` must already include
/// host, x-amz-date, and x-amz-content-sha256 (lowercase keys, trimmed
/// values); every header given is signed.
#[allow(clippy::too_many_arguments)]
pub fn authorization(
    key: &SigningKey<'_>,
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    headers: &BTreeMap<String, String>,
    payload_sha256_hex: &str,
    date_yyyymmdd: &str,
    timestamp_iso8601: &str,
) -> String {
    let canonical_headers = headers.iter().fold(String::new(), |mut acc, (k, v)| {
        use std::fmt::Write as _;
        let _ = writeln!(acc, "{k}:{v}");
        acc
    });
    let signed_headers: String = headers.keys().cloned().collect::<Vec<_>>().join(";");
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_sha256_hex}"
    );
    let scope = format!("{date_yyyymmdd}/{}/s3/aws4_request", key.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp_iso8601}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac(
        format!("AWS4{}", key.secret_key).as_bytes(),
        date_yyyymmdd.as_bytes(),
    );
    let k_region = hmac(&k_date, key.region.as_bytes());
    let k_service = hmac(&k_region, b"s3");
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}",
        key.access_key
    )
}

/// UTC timestamp pieces without a chrono dependency: civil-from-days
/// (Howard Hinnant's algorithm).
pub fn timestamp(now_unix: u64) -> (String, String) {
    let days = (now_unix / 86_400) as i64;
    let secs = now_unix % 86_400;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    let date = format!("{y:04}{mth:02}{d:02}");
    let iso = format!("{date}T{h:02}{m:02}{s:02}Z");
    (date, iso)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AWS's published SigV4 example: GET test.txt from examplebucket with
    /// the well-known example credentials. The expected signature is printed
    /// verbatim in the AWS documentation. Exact match or the signer is wrong.
    #[test]
    fn aws_published_get_object_vector() {
        let key = SigningKey {
            access_key: "AKIAIOSFODNN7EXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            region: "us-east-1",
        };
        let mut headers = BTreeMap::new();
        headers.insert("host".into(), "examplebucket.s3.amazonaws.com".into());
        headers.insert("range".into(), "bytes=0-9".into());
        headers.insert(
            "x-amz-content-sha256".into(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        );
        headers.insert("x-amz-date".into(), "20130524T000000Z".into());
        let auth = authorization(
            &key,
            "GET",
            "/test.txt",
            "",
            &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "20130524",
            "20130524T000000Z",
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn timestamp_math_is_civil() {
        // 2013-05-24T00:00:00Z == 1369353600 (the vector's date).
        assert_eq!(
            timestamp(1_369_353_600),
            ("20130524".into(), "20130524T000000Z".into())
        );
        // Epoch itself.
        assert_eq!(timestamp(0), ("19700101".into(), "19700101T000000Z".into()));
        // A leap-year date deep in the calendar: 2024-02-29T12:34:56Z.
        assert_eq!(
            timestamp(1_709_210_096),
            ("20240229".into(), "20240229T123456Z".into())
        );
    }

    #[test]
    fn uri_encoding_rules() {
        assert_eq!(uri_encode("objects/aa/ff00", true), "objects/aa/ff00");
        assert_eq!(uri_encode("a b+c", false), "a%20b%2Bc");
        assert_eq!(uri_encode("pre/fix", false), "pre%2Ffix");
    }
}
