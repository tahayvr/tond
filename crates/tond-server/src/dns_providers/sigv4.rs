// sigv4.rs — AWS Signature Version 4 for the Route 53 API
//
// Just enough of SigV4 for Route 53's REST API: a request with a path, an
// optional query and an optional body, signed with a static access key for
// region us-east-1 and service "route53". Avoids pulling in the AWS SDK.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// RFC 3986 percent-encoding of everything but unreserved characters.
pub fn uri_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What to sign.
pub struct SignInput<'a> {
    pub method: &'a str,
    pub host: &'a str,
    /// Absolute path, already URI-encoded (`/2013-04-01/hostedzone/Z1/rrset`).
    pub path: &'a str,
    /// Unencoded query parameters.
    pub query: &'a [(&'a str, &'a str)],
    /// Extra headers to sign besides host and x-amz-date (lowercase names).
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
    pub region: &'a str,
    pub service: &'a str,
    /// `YYYYMMDDTHHMMSSZ`
    pub amz_date: &'a str,
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
}

/// The canonical query string (sorted, encoded).
pub fn canonical_query(query: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (uri_encode(k), uri_encode(v)))
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// The `Authorization` header value.
pub fn authorization(input: &SignInput<'_>) -> String {
    let date = &input.amz_date[..8.min(input.amz_date.len())];
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), input.host.to_string()),
        ("x-amz-date".into(), input.amz_date.to_string()),
    ];
    headers.extend(
        input
            .headers
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())),
    );
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        input.method,
        input.path,
        canonical_query(input.query),
        canonical_headers,
        signed_headers,
        sha256_hex(input.body)
    );
    let scope = format!("{date}/{}/{}/aws4_request", input.region, input.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        input.amz_date,
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac(format!("AWS4{}", input.secret_access_key).as_bytes(), date);
    let k_region = hmac(&k_date, input.region);
    let k_service = hmac(&k_region, input.service);
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex::encode(hmac(&k_signing, &string_to_sign));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        input.access_key_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "get-vanilla" and "get-vanilla-query-order-key-case" from the AWS
    /// SigV4 test suite.
    #[test]
    fn matches_the_aws_test_suite() {
        let base = SignInput {
            method: "GET",
            host: "example.amazonaws.com",
            path: "/",
            query: &[],
            headers: &[],
            body: b"",
            region: "us-east-1",
            service: "service",
            amz_date: "20150830T123600Z",
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        };
        assert_eq!(
            authorization(&base),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        let query = SignInput {
            query: &[("Param2", "value2"), ("Param1", "value1")],
            ..base
        };
        assert!(
            authorization(&query).ends_with(
                "Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
            ),
            "{}",
            authorization(&query)
        );
    }

    #[test]
    fn query_values_are_sorted_and_encoded() {
        assert_eq!(
            canonical_query(&[("maxitems", "1"), ("dnsname", "a b.example.com")]),
            "dnsname=a%20b.example.com&maxitems=1"
        );
    }
}
