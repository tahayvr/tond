// clients.rs — DNS provider API clients (publish / remove a TXT record)
//
// Each provider speaks its own REST API; all of them go through a small
// `Transport` so the request formatting is unit-tested against a recording
// mock (no network in tests) and production uses one shared reqwest client.
//
//   Cloudflare     api.cloudflare.com/client/v4       Bearer token (Zone → DNS → Edit)
//   Hetzner        api.hetzner.cloud/v1 (DNS zones)    Bearer token (project, Read & Write)
//   DigitalOcean   api.digitalocean.com/v2            Bearer token (domain:create/read/delete)
//   Route 53       route53.amazonaws.com/2013-04-01   SigV4 (access key id + secret key)
//   challtestsrv   <url>/set-txt, /clear-txt          none (test only: Pebble's DNS)
//
// `Client::check_zone` verifies a token can see a zone (the "Test" button),
// `set_txt` / `clear_txt` publish and remove `_acme-challenge` records.

use std::future::Future;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tond_common::{DnsProviderConfig, DnsProviderKind};

use super::sigv4;

/// One HTTP request, provider-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

pub trait Transport: Sync {
    fn send(&self, req: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send;
}

/// Production transport: one shared reqwest client.
pub struct Reqwest;

static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("tond/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
});

impl Transport for Reqwest {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())?;
        let mut builder = HTTP.request(method, &req.url);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let resp = builder.send().await.context("calling the DNS provider")?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok(HttpResponse { status, body })
    }
}

const CLOUDFLARE: &str = "https://api.cloudflare.com/client/v4";
const HETZNER: &str = "https://api.hetzner.cloud/v1";
const DIGITALOCEAN: &str = "https://api.digitalocean.com/v2";
const ROUTE53_HOST: &str = "route53.amazonaws.com";
const DEFAULT_CHALLTESTSRV: &str = "http://127.0.0.1:8055";

/// TTL of challenge records (seconds; DigitalOcean's minimum is 30).
const TXT_TTL: u32 = 60;

/// `name` relative to `zone`: `@` for the apex.
pub fn relative_name(name: &str, zone: &str) -> String {
    if name == zone {
        return "@".into();
    }
    name.strip_suffix(&format!(".{zone}"))
        .unwrap_or(name)
        .to_string()
}

fn quoted(value: &str) -> String {
    format!("\"{value}\"")
}

/// A provider's error text from a failed response.
fn api_error(provider: &str, resp: &HttpResponse) -> anyhow::Error {
    let v = resp.json();
    let message = v["errors"][0]["message"]
        .as_str()
        .or_else(|| v["error"]["message"].as_str())
        .or_else(|| v["message"].as_str())
        .map(str::to_string)
        .or_else(|| xml_tag(&resp.body, "Message"))
        .unwrap_or_else(|| resp.body.chars().take(200).collect());
    let hint = match resp.status {
        401 | 403 => " — check the token and its permissions",
        404 => " — check the zone name",
        _ => "",
    };
    anyhow::anyhow!("{provider} answered {}: {message}{hint}", resp.status)
}

/// Text inside the first `<tag>…</tag>` (Route 53 answers in XML).
fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(xml[start..end].to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A provider client with its (decrypted) credentials.
pub struct Client {
    pub kind: DnsProviderKind,
    pub token: String,
    pub config: DnsProviderConfig,
}

impl Client {
    fn bearer(&self, method: &'static str, url: String, body: Option<Value>) -> HttpRequest {
        let mut headers = vec![("Authorization".into(), format!("Bearer {}", self.token))];
        if body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        HttpRequest {
            method,
            url,
            headers,
            body: body.map(|b| b.to_string()),
        }
    }

    fn challtestsrv_url(&self) -> String {
        self.config
            .url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CHALLTESTSRV.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    /// Sign a Route 53 request.
    fn route53(
        &self,
        method: &'static str,
        path: &str,
        query: &[(&str, &str)],
        body: Option<String>,
        amz_date: &str,
    ) -> HttpRequest {
        let access_key_id = self.config.access_key_id.clone().unwrap_or_default();
        let content_type = [("content-type", "text/xml")];
        let extra: &[(&str, &str)] = if body.is_some() { &content_type } else { &[] };
        let auth = sigv4::authorization(&sigv4::SignInput {
            method,
            host: ROUTE53_HOST,
            path,
            query,
            headers: extra,
            body: body.as_deref().unwrap_or_default().as_bytes(),
            region: "us-east-1",
            service: "route53",
            amz_date,
            access_key_id: &access_key_id,
            secret_access_key: &self.token,
        });
        let mut url = format!("https://{ROUTE53_HOST}{path}");
        if !query.is_empty() {
            url = format!("{url}?{}", sigv4::canonical_query(query));
        }
        let mut headers = vec![
            ("X-Amz-Date".into(), amz_date.to_string()),
            ("Authorization".into(), auth),
        ];
        if body.is_some() {
            headers.push(("Content-Type".into(), "text/xml".into()));
        }
        HttpRequest {
            method,
            url,
            headers,
            body,
        }
    }

    fn amz_date() -> String {
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
    }

    async fn cloudflare_zone_id<T: Transport>(&self, t: &T, zone: &str) -> Result<String> {
        let resp = t
            .send(self.bearer(
                "GET",
                format!("{CLOUDFLARE}/zones?name={}", urlencoding::encode(zone)),
                None,
            ))
            .await?;
        if !resp.ok() {
            return Err(api_error("Cloudflare", &resp));
        }
        resp.json()["result"][0]["id"]
            .as_str()
            .map(str::to_string)
            .with_context(|| {
                format!(
                    "Cloudflare has no zone {zone} this token can see (Zone → DNS → Edit on {zone})"
                )
            })
    }

    async fn route53_zone_id<T: Transport>(&self, t: &T, zone: &str) -> Result<String> {
        let path = "/2013-04-01/hostedzonesbyname";
        let resp = t
            .send(self.route53(
                "GET",
                path,
                &[("dnsname", zone), ("maxitems", "1")],
                None,
                &Self::amz_date(),
            ))
            .await?;
        if !resp.ok() {
            return Err(api_error("Route 53", &resp));
        }
        let name = xml_tag(&resp.body, "Name").unwrap_or_default();
        let id = xml_tag(&resp.body, "Id").unwrap_or_default();
        if name.trim_end_matches('.') != zone || id.is_empty() {
            bail!("Route 53 has no hosted zone {zone} these credentials can see");
        }
        Ok(id.trim_start_matches("/hostedzone/").to_string())
    }

    fn route53_change(action: &str, name: &str, value: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <ChangeResourceRecordSetsRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <ChangeBatch><Changes><Change><Action>{action}</Action><ResourceRecordSet>\
             <Name>{}.</Name><Type>TXT</Type><TTL>{TXT_TTL}</TTL>\
             <ResourceRecords><ResourceRecord><Value>{}</Value></ResourceRecord></ResourceRecords>\
             </ResourceRecordSet></Change></Changes></ChangeBatch></ChangeResourceRecordSetsRequest>",
            xml_escape(name),
            xml_escape(&quoted(value))
        )
    }

    /// Check that the credentials can see `zone`.
    pub async fn check_zone<T: Transport>(&self, t: &T, zone: &str) -> Result<()> {
        match self.kind {
            DnsProviderKind::Cloudflare => self.cloudflare_zone_id(t, zone).await.map(|_| ()),
            DnsProviderKind::Hetzner => {
                let resp = t
                    .send(self.bearer("GET", format!("{HETZNER}/zones/{zone}"), None))
                    .await?;
                if resp.ok() {
                    Ok(())
                } else {
                    Err(api_error("Hetzner", &resp))
                }
            }
            DnsProviderKind::Digitalocean => {
                let resp = t
                    .send(self.bearer("GET", format!("{DIGITALOCEAN}/domains/{zone}"), None))
                    .await?;
                if resp.ok() {
                    Ok(())
                } else {
                    Err(api_error("DigitalOcean", &resp))
                }
            }
            DnsProviderKind::Route53 => self.route53_zone_id(t, zone).await.map(|_| ()),
            DnsProviderKind::Challtestsrv => Ok(()),
        }
    }

    /// Publish a TXT record `name` = `value` in `zone`.
    pub async fn set_txt<T: Transport>(
        &self,
        t: &T,
        zone: &str,
        name: &str,
        value: &str,
    ) -> Result<()> {
        let (provider, req) = match self.kind {
            DnsProviderKind::Cloudflare => {
                let id = self.cloudflare_zone_id(t, zone).await?;
                (
                    "Cloudflare",
                    self.bearer(
                        "POST",
                        format!("{CLOUDFLARE}/zones/{id}/dns_records"),
                        Some(json!({
                            "type": "TXT",
                            "name": name,
                            "content": quoted(value),
                            "ttl": TXT_TTL,
                            "comment": "Tond ACME challenge",
                        })),
                    ),
                )
            }
            DnsProviderKind::Hetzner => (
                "Hetzner",
                self.bearer(
                    "POST",
                    format!(
                        "{HETZNER}/zones/{zone}/rrsets/{}/TXT/actions/add_records",
                        relative_name(name, zone)
                    ),
                    Some(json!({ "ttl": TXT_TTL, "records": [{ "value": quoted(value) }] })),
                ),
            ),
            DnsProviderKind::Digitalocean => (
                "DigitalOcean",
                self.bearer(
                    "POST",
                    format!("{DIGITALOCEAN}/domains/{zone}/records"),
                    Some(json!({
                        "type": "TXT",
                        "name": relative_name(name, zone),
                        "data": value,
                        "ttl": TXT_TTL,
                    })),
                ),
            ),
            DnsProviderKind::Route53 => {
                let id = self.route53_zone_id(t, zone).await?;
                (
                    "Route 53",
                    self.route53(
                        "POST",
                        &format!("/2013-04-01/hostedzone/{id}/rrset"),
                        &[],
                        Some(Self::route53_change("UPSERT", name, value)),
                        &Self::amz_date(),
                    ),
                )
            }
            DnsProviderKind::Challtestsrv => (
                "challtestsrv",
                HttpRequest {
                    method: "POST",
                    url: format!("{}/set-txt", self.challtestsrv_url()),
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: Some(json!({ "host": format!("{name}."), "value": value }).to_string()),
                },
            ),
        };
        let resp = t.send(req).await?;
        if !resp.ok() {
            return Err(api_error(provider, &resp));
        }
        Ok(())
    }

    /// Remove the TXT record `name` = `value` from `zone`. A record that is
    /// already gone is not an error.
    pub async fn clear_txt<T: Transport>(
        &self,
        t: &T,
        zone: &str,
        name: &str,
        value: &str,
    ) -> Result<()> {
        match self.kind {
            DnsProviderKind::Cloudflare => {
                let id = self.cloudflare_zone_id(t, zone).await?;
                let resp = t
                    .send(self.bearer(
                        "GET",
                        format!(
                            "{CLOUDFLARE}/zones/{id}/dns_records?type=TXT&name={}",
                            urlencoding::encode(name)
                        ),
                        None,
                    ))
                    .await?;
                if !resp.ok() {
                    return Err(api_error("Cloudflare", &resp));
                }
                let records = resp.json()["result"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for record in records.iter().filter(|r| {
                    let content = r["content"].as_str().unwrap_or_default();
                    content == value || content == quoted(value)
                }) {
                    let Some(rid) = record["id"].as_str() else {
                        continue;
                    };
                    let resp = t
                        .send(self.bearer(
                            "DELETE",
                            format!("{CLOUDFLARE}/zones/{id}/dns_records/{rid}"),
                            None,
                        ))
                        .await?;
                    if !resp.ok() && resp.status != 404 {
                        return Err(api_error("Cloudflare", &resp));
                    }
                }
                Ok(())
            }
            DnsProviderKind::Hetzner => {
                let resp = t
                    .send(self.bearer(
                        "POST",
                        format!(
                            "{HETZNER}/zones/{zone}/rrsets/{}/TXT/actions/remove_records",
                            relative_name(name, zone)
                        ),
                        Some(json!({ "records": [{ "value": quoted(value) }] })),
                    ))
                    .await?;
                if !resp.ok() && resp.status != 404 {
                    return Err(api_error("Hetzner", &resp));
                }
                Ok(())
            }
            DnsProviderKind::Digitalocean => {
                let resp = t
                    .send(self.bearer(
                        "GET",
                        format!(
                            "{DIGITALOCEAN}/domains/{zone}/records?type=TXT&name={}&per_page=200",
                            urlencoding::encode(name)
                        ),
                        None,
                    ))
                    .await?;
                if !resp.ok() {
                    return Err(api_error("DigitalOcean", &resp));
                }
                let records = resp.json()["domain_records"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for record in records.iter().filter(|r| r["data"].as_str() == Some(value)) {
                    let Some(rid) = record["id"].as_i64() else {
                        continue;
                    };
                    let resp = t
                        .send(self.bearer(
                            "DELETE",
                            format!("{DIGITALOCEAN}/domains/{zone}/records/{rid}"),
                            None,
                        ))
                        .await?;
                    if !resp.ok() && resp.status != 404 {
                        return Err(api_error("DigitalOcean", &resp));
                    }
                }
                Ok(())
            }
            DnsProviderKind::Route53 => {
                let id = self.route53_zone_id(t, zone).await?;
                let resp = t
                    .send(self.route53(
                        "POST",
                        &format!("/2013-04-01/hostedzone/{id}/rrset"),
                        &[],
                        Some(Self::route53_change("DELETE", name, value)),
                        &Self::amz_date(),
                    ))
                    .await?;
                // Deleting a record that no longer exists is InvalidChangeBatch.
                if !resp.ok() && !resp.body.contains("not found") {
                    return Err(api_error("Route 53", &resp));
                }
                Ok(())
            }
            DnsProviderKind::Challtestsrv => {
                let resp = t
                    .send(HttpRequest {
                        method: "POST",
                        url: format!("{}/clear-txt", self.challtestsrv_url()),
                        headers: vec![("Content-Type".into(), "application/json".into())],
                        body: Some(json!({ "host": format!("{name}.") }).to_string()),
                    })
                    .await?;
                if !resp.ok() {
                    return Err(api_error("challtestsrv", &resp));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
pub mod mock {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    /// Records every request and answers from a queue (200 `{}` when empty).
    #[derive(Default)]
    pub struct Mock {
        pub requests: Mutex<Vec<HttpRequest>>,
        pub responses: Mutex<VecDeque<HttpResponse>>,
    }

    impl Mock {
        pub fn answer(self, status: u16, body: &str) -> Self {
            self.responses
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(HttpResponse {
                    status,
                    body: body.to_string(),
                });
            self
        }

        pub fn sent(&self) -> Vec<HttpRequest> {
            self.requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    impl Transport for Mock {
        async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
            self.requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(req);
            Ok(self
                .responses
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
                .unwrap_or(HttpResponse {
                    status: 200,
                    body: "{}".into(),
                }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::Mock;
    use super::*;

    fn client(kind: DnsProviderKind) -> Client {
        Client {
            kind,
            token: "tok".into(),
            config: DnsProviderConfig {
                access_key_id: Some("AKID".into()),
                url: None,
            },
        }
    }

    fn body(req: &HttpRequest) -> Value {
        serde_json::from_str(req.body.as_deref().unwrap_or("null")).unwrap_or(Value::Null)
    }

    fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    const NAME: &str = "_acme-challenge.apps.example.com";

    #[tokio::test]
    async fn cloudflare_looks_up_the_zone_then_creates_and_deletes_the_record() {
        let t = Mock::default().answer(200, r#"{"success":true,"result":[{"id":"z1"}]}"#);
        client(DnsProviderKind::Cloudflare)
            .set_txt(&t, "example.com", NAME, "v4lue")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let sent = t.sent();
        assert_eq!(sent[0].method, "GET");
        assert_eq!(sent[0].url, format!("{CLOUDFLARE}/zones?name=example.com"));
        assert_eq!(header(&sent[0], "authorization"), Some("Bearer tok"));
        assert_eq!(sent[1].method, "POST");
        assert_eq!(sent[1].url, format!("{CLOUDFLARE}/zones/z1/dns_records"));
        let b = body(&sent[1]);
        assert_eq!(b["type"], "TXT");
        assert_eq!(b["name"], NAME);
        assert_eq!(b["content"], "\"v4lue\"");
        assert_eq!(b["ttl"], 60);

        let t = Mock::default()
            .answer(200, r#"{"result":[{"id":"z1"}]}"#)
            .answer(
                200,
                r#"{"result":[{"id":"r1","content":"\"v4lue\""},{"id":"r2","content":"other"}]}"#,
            );
        client(DnsProviderKind::Cloudflare)
            .clear_txt(&t, "example.com", NAME, "v4lue")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let sent = t.sent();
        assert_eq!(sent.len(), 3, "zone, list, one delete");
        assert!(
            sent[1]
                .url
                .contains("type=TXT&name=_acme-challenge.apps.example.com")
        );
        assert_eq!(sent[2].method, "DELETE");
        assert_eq!(sent[2].url, format!("{CLOUDFLARE}/zones/z1/dns_records/r1"));
    }

    #[tokio::test]
    async fn cloudflare_errors_explain_the_token() {
        let t = Mock::default().answer(
            403,
            r#"{"success":false,"errors":[{"code":9109,"message":"Unauthorized to access requested resource"}]}"#,
        );
        let err = client(DnsProviderKind::Cloudflare)
            .check_zone(&t, "example.com")
            .await
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("Unauthorized to access requested resource"),
            "{err}"
        );
        assert!(err.contains("check the token"), "{err}");

        let t = Mock::default().answer(200, r#"{"success":true,"result":[]}"#);
        assert!(
            client(DnsProviderKind::Cloudflare)
                .check_zone(&t, "example.com")
                .await
                .is_err(),
            "an invisible zone is an error"
        );
    }

    #[tokio::test]
    async fn hetzner_adds_and_removes_records_on_the_rrset() {
        let t = Mock::default();
        let c = client(DnsProviderKind::Hetzner);
        c.set_txt(&t, "example.com", NAME, "v")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        c.clear_txt(&t, "example.com", NAME, "v")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let sent = t.sent();
        assert_eq!(
            sent[0].url,
            format!(
                "{HETZNER}/zones/example.com/rrsets/_acme-challenge.apps/TXT/actions/add_records"
            )
        );
        assert_eq!(body(&sent[0])["records"][0]["value"], "\"v\"");
        assert_eq!(body(&sent[0])["ttl"], 60);
        assert!(sent[1].url.ends_with("/TXT/actions/remove_records"));
        assert_eq!(header(&sent[1], "authorization"), Some("Bearer tok"));
    }

    #[tokio::test]
    async fn digitalocean_uses_relative_names_and_deletes_by_id() {
        let t = Mock::default();
        let c = client(DnsProviderKind::Digitalocean);
        c.set_txt(&t, "example.com", NAME, "v")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let sent = t.sent();
        assert_eq!(
            sent[0].url,
            format!("{DIGITALOCEAN}/domains/example.com/records")
        );
        let b = body(&sent[0]);
        assert_eq!(
            (b["type"].as_str(), b["name"].as_str(), b["data"].as_str()),
            (Some("TXT"), Some("_acme-challenge.apps"), Some("v"))
        );

        let t = Mock::default().answer(
            200,
            r#"{"domain_records":[{"id":11,"data":"v"},{"id":12,"data":"w"}]}"#,
        );
        c.clear_txt(&t, "example.com", NAME, "v")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let sent = t.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].method, "DELETE");
        assert_eq!(
            sent[1].url,
            format!("{DIGITALOCEAN}/domains/example.com/records/11")
        );
    }

    #[tokio::test]
    async fn route53_finds_the_hosted_zone_and_signs_an_upsert() {
        let t = Mock::default().answer(
            200,
            "<ListHostedZonesByNameResponse><HostedZones><HostedZone><Id>/hostedzone/Z123</Id>\
             <Name>example.com.</Name></HostedZone></HostedZones></ListHostedZonesByNameResponse>",
        );
        client(DnsProviderKind::Route53)
            .set_txt(&t, "example.com", NAME, "v")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let sent = t.sent();
        assert_eq!(
            sent[0].url,
            "https://route53.amazonaws.com/2013-04-01/hostedzonesbyname?dnsname=example.com&maxitems=1"
        );
        assert_eq!(
            sent[1].url,
            "https://route53.amazonaws.com/2013-04-01/hostedzone/Z123/rrset"
        );
        let auth = header(&sent[1], "authorization").unwrap_or_default();
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKID/"),
            "{auth}"
        );
        assert!(auth.contains("/us-east-1/route53/aws4_request"), "{auth}");
        assert!(
            auth.contains("SignedHeaders=content-type;host;x-amz-date"),
            "{auth}"
        );
        let xml = sent[1].body.clone().unwrap_or_default();
        assert!(xml.contains("<Action>UPSERT</Action>"));
        assert!(xml.contains("<Name>_acme-challenge.apps.example.com.</Name>"));
        assert!(xml.contains("<Value>&quot;v&quot;</Value>"));

        let t = Mock::default().answer(
            200,
            "<HostedZone><Id>/hostedzone/Z9</Id><Name>other.com.</Name></HostedZone>",
        );
        assert!(
            client(DnsProviderKind::Route53)
                .check_zone(&t, "example.com")
                .await
                .is_err(),
            "the first zone by name must be the one asked for"
        );
    }

    #[tokio::test]
    async fn challtestsrv_sets_and_clears_by_host() {
        let t = Mock::default();
        let c = Client {
            kind: DnsProviderKind::Challtestsrv,
            token: String::new(),
            config: DnsProviderConfig {
                access_key_id: None,
                url: Some("http://127.0.0.1:8055/".into()),
            },
        };
        c.set_txt(&t, "apps.test", "_acme-challenge.apps.test", "v")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        c.clear_txt(&t, "apps.test", "_acme-challenge.apps.test", "v")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let sent = t.sent();
        assert_eq!(sent[0].url, "http://127.0.0.1:8055/set-txt");
        assert_eq!(body(&sent[0])["host"], "_acme-challenge.apps.test.");
        assert_eq!(body(&sent[0])["value"], "v");
        assert_eq!(sent[1].url, "http://127.0.0.1:8055/clear-txt");
    }

    #[test]
    fn relative_names() {
        assert_eq!(
            relative_name("_acme-challenge.example.com", "example.com"),
            "_acme-challenge"
        );
        assert_eq!(relative_name("example.com", "example.com"), "@");
        assert_eq!(relative_name("a.b.example.com", "example.com"), "a.b");
    }
}
