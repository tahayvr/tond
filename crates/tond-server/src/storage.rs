// storage.rs — Remote object storage abstraction (S3-compatible SigV4)
//
// Public API:
//   upload(provider, key, path)   — upload a local file as an S3 object
//   download(provider, key, dest) — download an S3 object to a local file
//   list(provider, prefix)        — list objects with the given key prefix
//   delete(provider, key)         — delete an S3 object
//   test_connectivity(provider)   — attempt a ListObjects call; Ok = connected
//
// All operations are implemented using reqwest + SigV4 signing built from the
// existing sha2 / hmac / hex / base64 workspace crates. No AWS SDK dependency.
//
// The `StorageProvider` passed in must already have the secret decrypted
// (use `handlers::storage_providers::decrypt_provider_secret` before calling).

use std::path::Path;

use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tond_common::StorageProvider;

type HmacSha256 = Hmac<Sha256>;

// ── Public surface ────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct RemoteObject {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
}

/// Upload a local file at `path` to the remote storage under `key`.
pub async fn upload(provider: &StorageProvider, key: &str, path: &Path) -> anyhow::Result<()> {
    let cfg = parse_config(provider)?;
    let mut f = File::open(path).await?;
    let mut body = Vec::new();
    f.read_to_end(&mut body).await?;

    let (endpoint, host) = endpoint_and_host(&cfg);
    let url = format!("{}/{}/{}", endpoint, cfg.bucket, key);
    let content_type = "application/octet-stream";

    let now = Utc::now();
    let date_time = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_short = now.format("%Y%m%d").to_string();

    let payload_hash = hex_sha256(&body);

    let headers = vec![
        ("content-type", content_type.to_string()),
        ("host", host.clone()),
        ("x-amz-content-sha256", payload_hash.clone()),
        ("x-amz-date", date_time.clone()),
    ];

    let signed = sigv4_sign(
        "PUT",
        &url_path(&cfg.bucket, key),
        "",
        &headers,
        &payload_hash,
        &date_time,
        &date_short,
        &cfg.region,
        &cfg.access_key_id,
        &cfg.secret_access_key,
    )?;

    let client = Client::new();
    let resp = client
        .put(&url)
        .header("content-type", content_type)
        .header("host", &host)
        .header("x-amz-content-sha256", &payload_hash)
        .header("x-amz-date", &date_time)
        .header("authorization", &signed)
        .body(body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("S3 upload failed ({status}): {body}");
    }
    Ok(())
}

/// Download the object at `key` from remote storage to local `dest`.
pub async fn download(provider: &StorageProvider, key: &str, dest: &Path) -> anyhow::Result<()> {
    let cfg = parse_config(provider)?;
    let (endpoint, host) = endpoint_and_host(&cfg);
    let url = format!("{}/{}/{}", endpoint, cfg.bucket, key);

    let now = Utc::now();
    let date_time = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_short = now.format("%Y%m%d").to_string();
    let payload_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"; // empty body

    let headers = vec![
        ("host", host.clone()),
        ("x-amz-content-sha256", payload_hash.to_string()),
        ("x-amz-date", date_time.clone()),
    ];

    let signed = sigv4_sign(
        "GET",
        &url_path(&cfg.bucket, key),
        "",
        &headers,
        payload_hash,
        &date_time,
        &date_short,
        &cfg.region,
        &cfg.access_key_id,
        &cfg.secret_access_key,
    )?;

    let client = Client::new();
    let resp = client
        .get(&url)
        .header("host", &host)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", &date_time)
        .header("authorization", &signed)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("S3 download failed ({status}): {body_text}");
    }

    let bytes = resp.bytes().await?;
    tokio::fs::write(dest, &bytes).await?;
    Ok(())
}

/// List objects under the given `prefix` in the configured bucket.
pub async fn list(provider: &StorageProvider, prefix: &str) -> anyhow::Result<Vec<RemoteObject>> {
    let cfg = parse_config(provider)?;
    let (endpoint, host) = endpoint_and_host(&cfg);

    let effective_prefix = match &cfg.prefix {
        Some(p) => format!("{}{}", p, prefix),
        None => prefix.to_string(),
    };
    let query = format!(
        "list-type=2&prefix={}",
        urlencoding::encode(&effective_prefix)
    );
    let url = format!("{}/{}?{}", endpoint, cfg.bucket, query);

    let now = Utc::now();
    let date_time = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_short = now.format("%Y%m%d").to_string();
    let payload_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    let headers = vec![
        ("host", host.clone()),
        ("x-amz-content-sha256", payload_hash.to_string()),
        ("x-amz-date", date_time.clone()),
    ];

    let signed = sigv4_sign(
        "GET",
        &format!("/{}", cfg.bucket),
        &query,
        &headers,
        payload_hash,
        &date_time,
        &date_short,
        &cfg.region,
        &cfg.access_key_id,
        &cfg.secret_access_key,
    )?;

    let client = Client::new();
    let resp = client
        .get(&url)
        .header("host", &host)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", &date_time)
        .header("authorization", &signed)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("S3 list failed ({status}): {body_text}");
    }

    let xml = resp.text().await?;
    parse_list_objects_response(&xml)
}

/// Delete the object at `key` from remote storage.
pub async fn delete(provider: &StorageProvider, key: &str) -> anyhow::Result<()> {
    let cfg = parse_config(provider)?;
    let (endpoint, host) = endpoint_and_host(&cfg);
    let url = format!("{}/{}/{}", endpoint, cfg.bucket, key);

    let now = Utc::now();
    let date_time = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_short = now.format("%Y%m%d").to_string();
    let payload_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    let headers = vec![
        ("host", host.clone()),
        ("x-amz-content-sha256", payload_hash.to_string()),
        ("x-amz-date", date_time.clone()),
    ];

    let signed = sigv4_sign(
        "DELETE",
        &url_path(&cfg.bucket, key),
        "",
        &headers,
        payload_hash,
        &date_time,
        &date_short,
        &cfg.region,
        &cfg.access_key_id,
        &cfg.secret_access_key,
    )?;

    let client = Client::new();
    let resp = client
        .delete(&url)
        .header("host", &host)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", &date_time)
        .header("authorization", &signed)
        .send()
        .await?;

    let status = resp.status();
    // 204 No Content is the expected success for DELETE
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("S3 delete failed ({status}): {body_text}");
    }
    Ok(())
}

/// Verify connectivity by issuing a ListObjects request. Returns Ok(()) if
/// the bucket is reachable and the credentials are valid.
pub async fn test_connectivity(provider: &StorageProvider) -> anyhow::Result<()> {
    list(provider, "").await?;
    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Parse the provider config, expecting the secret to already be decrypted.
fn parse_config(provider: &StorageProvider) -> anyhow::Result<S3ConfigDecrypted> {
    let cfg: serde_json::Value = serde_json::from_str(&provider.config)?;
    let obj = cfg
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid config"))?;

    Ok(S3ConfigDecrypted {
        endpoint: obj
            .get("endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        region: obj
            .get("region")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing region"))?
            .to_string(),
        bucket: obj
            .get("bucket")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing bucket"))?
            .to_string(),
        prefix: obj
            .get("prefix")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        access_key_id: obj
            .get("access_key_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing access_key_id"))?
            .to_string(),
        // Must be decrypted already (secret_access_key — not _enc)
        secret_access_key: obj
            .get("secret_access_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing secret_access_key (did you decrypt?)"))?
            .to_string(),
    })
}

struct S3ConfigDecrypted {
    endpoint: Option<String>,
    region: String,
    bucket: String,
    prefix: Option<String>,
    access_key_id: String,
    secret_access_key: String,
}

/// Returns (endpoint_url, host_header).
fn endpoint_and_host(cfg: &S3ConfigDecrypted) -> (String, String) {
    match &cfg.endpoint {
        Some(ep) => {
            let endpoint = ep.trim_end_matches('/').to_string();
            let host = endpoint
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_string();
            (endpoint, host)
        }
        None => {
            let host = format!("s3.{}.amazonaws.com", cfg.region);
            let ep = format!("https://{}", host);
            (ep, host)
        }
    }
}

fn url_path(bucket: &str, key: &str) -> String {
    format!("/{}/{}", bucket, key)
}

fn hex_sha256(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hex::encode(hash)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Compute an AWS SigV4 `Authorization` header value.
///
/// `headers` must be sorted alphabetically by header name and must include
/// `host`, `x-amz-content-sha256`, and `x-amz-date`.
#[allow(clippy::too_many_arguments)]
fn sigv4_sign(
    method: &str,
    canonical_uri: &str,
    canonical_querystring: &str,
    headers: &[(&str, String)],
    payload_hash: &str,
    date_time: &str,  // "20260101T120000Z"
    date_short: &str, // "20260101"
    region: &str,
    access_key_id: &str,
    secret_key: &str,
) -> anyhow::Result<String> {
    // 1. Canonical headers + signed headers list
    let mut sorted_headers: Vec<(&str, &str)> =
        headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
    sorted_headers.sort_by_key(|(k, _)| *k);

    let canonical_headers = sorted_headers
        .iter()
        .map(|(k, v)| format!("{}:{}\n", k.to_lowercase(), v.trim()))
        .collect::<String>();

    let signed_headers = sorted_headers
        .iter()
        .map(|(k, _)| k.to_lowercase())
        .collect::<Vec<_>>()
        .join(";");

    // 2. Canonical request
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_querystring}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    // 3. String to sign
    let service = "s3";
    let scope = format!("{date_short}/{region}/{service}/aws4_request");
    let hashed_cr = hex_sha256(canonical_request.as_bytes());
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{date_time}\n{scope}\n{hashed_cr}");

    // 4. Signing key
    let k_secret = format!("AWS4{secret_key}");
    let k_date = hmac_sha256(k_secret.as_bytes(), date_short.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");

    // 5. Signature
    let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    ))
}

/// Minimal XML parser for S3 ListObjectsV2 response.
/// Extracts `<Key>`, `<Size>`, and `<LastModified>` from `<Contents>` elements.
fn parse_list_objects_response(xml: &str) -> anyhow::Result<Vec<RemoteObject>> {
    let mut objects = Vec::new();
    let mut current_key = String::new();
    let mut current_size = 0u64;
    let mut current_last_modified = String::new();
    let mut in_contents = false;

    for line in xml.lines() {
        let trimmed = line.trim();
        if trimmed.contains("<Contents>") {
            in_contents = true;
            current_key.clear();
            current_size = 0;
            current_last_modified.clear();
        } else if trimmed.contains("</Contents>") {
            if in_contents && !current_key.is_empty() {
                objects.push(RemoteObject {
                    key: current_key.clone(),
                    size: current_size,
                    last_modified: current_last_modified.clone(),
                });
            }
            in_contents = false;
        } else if in_contents {
            if let Some(key) = extract_xml_value(trimmed, "Key") {
                current_key = key;
            } else if let Some(size) = extract_xml_value(trimmed, "Size") {
                current_size = size.parse().unwrap_or(0);
            } else if let Some(lm) = extract_xml_value(trimmed, "LastModified") {
                current_last_modified = lm;
            }
        }
    }

    Ok(objects)
}

/// Extract the text content of a simple XML element from a trimmed line.
/// e.g. extract_xml_value("<Key>foo/bar.tar.gz</Key>", "Key") -> Some("foo/bar.tar.gz")
fn extract_xml_value(line: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = line.find(&open)?;
    let end = line.find(&close)?;
    let value_start = start + open.len();
    if value_start >= end {
        return None;
    }
    Some(line[value_start..end].to_string())
}

#[cfg(test)]
mod tests {
    use tond_common::StorageProvider;

    use super::*;

    fn provider_with_config(config: serde_json::Value) -> StorageProvider {
        StorageProvider {
            id: "provider-1".to_string(),
            name: "Backups".to_string(),
            kind: "s3".to_string(),
            config: config.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn parse_config_accepts_valid_decrypted_s3_config() {
        let provider = provider_with_config(serde_json::json!({
            "endpoint": "https://r2.example.com",
            "region": "auto",
            "bucket": "tond-backups",
            "prefix": "snapshots/",
            "access_key_id": "AKIA123",
            "secret_access_key": "secret"
        }));

        let cfg = parse_config(&provider)
            .unwrap_or_else(|err| panic!("expected parse_config to succeed: {err}"));

        assert_eq!(
            cfg.endpoint.as_deref(),
            Some("https://r2.example.com"),
            "parse_config should preserve endpoint"
        );
        assert_eq!(cfg.region, "auto", "parse_config should preserve region");
        assert_eq!(
            cfg.bucket, "tond-backups",
            "parse_config should preserve bucket"
        );
        assert_eq!(
            cfg.prefix.as_deref(),
            Some("snapshots/"),
            "parse_config should preserve prefix"
        );
        assert_eq!(
            cfg.access_key_id, "AKIA123",
            "parse_config should preserve access key id"
        );
        assert_eq!(
            cfg.secret_access_key, "secret",
            "parse_config should require the decrypted secret_access_key field"
        );
    }

    #[test]
    fn parse_config_rejects_missing_required_fields() {
        let provider = provider_with_config(serde_json::json!({
            "bucket": "tond-backups",
            "access_key_id": "AKIA123"
        }));

        let error = parse_config(&provider)
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("expected parse_config to fail"));

        assert!(
            error.to_string().contains("missing region")
                || error.to_string().contains("missing secret_access_key"),
            "parse_config should fail when required decrypted S3 fields are missing, got: {error}"
        );
    }

    #[test]
    fn endpoint_and_host_use_custom_endpoint_or_aws_default() {
        let custom = S3ConfigDecrypted {
            endpoint: Some("https://r2.example.com/".to_string()),
            region: "auto".to_string(),
            bucket: "tond-backups".to_string(),
            prefix: None,
            access_key_id: "AKIA123".to_string(),
            secret_access_key: "secret".to_string(),
        };
        assert_eq!(
            endpoint_and_host(&custom),
            (
                "https://r2.example.com".to_string(),
                "r2.example.com".to_string()
            ),
            "endpoint_and_host should trim trailing slashes and derive the custom host header"
        );

        let aws = S3ConfigDecrypted {
            endpoint: None,
            region: "us-east-1".to_string(),
            bucket: "tond-backups".to_string(),
            prefix: None,
            access_key_id: "AKIA123".to_string(),
            secret_access_key: "secret".to_string(),
        };
        assert_eq!(
            endpoint_and_host(&aws),
            (
                "https://s3.us-east-1.amazonaws.com".to_string(),
                "s3.us-east-1.amazonaws.com".to_string()
            ),
            "endpoint_and_host should derive the standard AWS regional endpoint when endpoint is omitted"
        );
    }

    #[test]
    fn helper_functions_build_paths_and_hashes() {
        assert_eq!(
            url_path("bucket", "snapshots/demo.tar.gz"),
            "/bucket/snapshots/demo.tar.gz",
            "url_path should compose bucket and key into an S3 object path"
        );
        assert_eq!(
            hex_sha256(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            "hex_sha256 should compute the SHA-256 digest in lowercase hex"
        );
        assert_eq!(
            hex::encode(hmac_sha256(b"key", b"payload")),
            "5d98b45c90a207fa998ce639fea6f02ecc8cc3f36fef81d694fb856b4d0a28ca",
            "hmac_sha256 should compute HMAC-SHA256 bytes deterministically"
        );
    }

    #[test]
    fn sigv4_sign_returns_expected_authorization_format() {
        let auth = sigv4_sign(
            "GET",
            "/bucket/snapshots/demo.tar.gz",
            "list-type=2&prefix=snapshots%2F",
            &[
                ("host", "s3.us-east-1.amazonaws.com".to_string()),
                (
                    "x-amz-content-sha256",
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
                ),
                ("x-amz-date", "20260101T120000Z".to_string()),
            ],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "20260101T120000Z",
            "20260101",
            "us-east-1",
            "AKIA123",
            "secret",
        )
        .unwrap_or_else(|err| panic!("expected sigv4_sign to succeed: {err}"));

        assert!(
            auth.starts_with(
                "AWS4-HMAC-SHA256 Credential=AKIA123/20260101/us-east-1/s3/aws4_request"
            ),
            "sigv4_sign should start with the expected credential scope, got: {auth}"
        );
        assert!(
            auth.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"),
            "sigv4_sign should include the canonical signed headers, got: {auth}"
        );
        assert!(
            auth.contains("Signature="),
            "sigv4_sign should include a Signature component, got: {auth}"
        );
    }

    #[test]
    fn parse_list_objects_response_extracts_contents_and_ignores_missing_fields() {
        let xml = r#"
            <ListBucketResult>
              <Contents>
                <Key>snapshots/a.tar.gz</Key>
                <LastModified>2026-01-01T00:00:00.000Z</LastModified>
                <Size>123</Size>
              </Contents>
              <Contents>
                <Key>snapshots/b.tar.gz</Key>
                <LastModified>2026-01-02T00:00:00.000Z</LastModified>
                <Size>456</Size>
              </Contents>
              <Contents>
                <LastModified>2026-01-03T00:00:00.000Z</LastModified>
                <Size>789</Size>
              </Contents>
            </ListBucketResult>
        "#;

        let objects = parse_list_objects_response(xml)
            .unwrap_or_else(|err| panic!("expected parse_list_objects_response to succeed: {err}"));

        assert_eq!(
            objects.len(),
            2,
            "parse_list_objects_response should only emit entries that contain a Key"
        );
        assert_eq!(
            objects[0].key, "snapshots/a.tar.gz",
            "first object key should be parsed correctly"
        );
        assert_eq!(
            objects[0].size, 123,
            "first object size should be parsed correctly"
        );
        assert_eq!(
            objects[1].last_modified, "2026-01-02T00:00:00.000Z",
            "second object last_modified should be parsed correctly"
        );
    }

    #[test]
    fn extract_xml_value_returns_inner_text_or_none() {
        assert_eq!(
            extract_xml_value("<Key>snapshots/demo.tar.gz</Key>", "Key"),
            Some("snapshots/demo.tar.gz".to_string()),
            "extract_xml_value should return the element inner text for matching tags"
        );
        assert_eq!(
            extract_xml_value("<Key></Key>", "Key"),
            None,
            "extract_xml_value should return None for empty elements"
        );
        assert_eq!(
            extract_xml_value("<Other>value</Other>", "Key"),
            None,
            "extract_xml_value should return None when the requested tag is absent"
        );
    }
}
