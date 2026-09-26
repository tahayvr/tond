# Domains and certificates

Tond serves everything over HTTPS through tond-proxy, on ports 80 and 443 of your server. Certificates come from Let's Encrypt and are obtained and renewed automatically: one per hostname, or one wildcard for many. You can also upload your own. This page explains the kinds of domains Tond uses, how an app gets several domains and redirects, what DNS they need, and how certificates are issued.

## Three domains, three sites

| Domain | What it serves | Hostnames | Installer default |
| --- | --- | --- | --- |
| Dashboard domain | The dashboard and the API | Exactly this name | `<server-ip-with-dashes>.sslip.io` |
| Apps domain | Automatic app hostnames | `<app>.<apps domain>` | `<server-ip-with-dashes>.nip.io` |
| Preview domain | Pull-request previews | `<slug>-<app>.<preview domain>` | None: previews are off until you set one |

Apps run your code and previews run pull-request code. So the apps domain and the preview domain must each be a **different site** (registrable domain) from the dashboard, and from each other. Otherwise an app could send same-site requests to the dashboard API or set cookies the dashboard reads. Tond checks this with the Public Suffix List and refuses a domain that breaks the rule. See [Previews](/concepts/previews) for the full reasoning.

<figure class="diagram">
<svg viewBox="0 0 760 250" role="img" aria-labelledby="sites-title sites-desc" xmlns="http://www.w3.org/2000/svg">
<title id="sites-title">Dashboard, apps and previews on three separate sites</title>
<desc id="sites-desc">Three boxes, one per registrable domain. The dashboard site (tond.example.com) serves the dashboard and the API and holds the session cookie. The apps site (example-apps.com) serves your apps at app.example-apps.com. The previews site (example-previews.com) serves pull-request previews at slug-app.example-previews.com. A browser treats requests between the three as cross-site: no shared cookies, and the dashboard's CSRF checks reject them.</desc>
<g font-size="13" fill="var(--foreground)">
<rect x="10" y="20" width="230" height="170" rx="8" fill="var(--card)" stroke="var(--foreground)" stroke-width="1.5"/>
<text x="125" y="48" text-anchor="middle" font-weight="600">Dashboard site</text>
<text x="125" y="70" text-anchor="middle" fill="var(--muted-foreground)" font-size="12">tond.example.com</text>
<rect x="35" y="90" width="180" height="36" rx="5" fill="var(--muted)"/>
<text x="125" y="113" text-anchor="middle" font-size="12">dashboard + API</text>
<rect x="35" y="136" width="180" height="36" rx="5" fill="var(--muted)"/>
<text x="125" y="159" text-anchor="middle" font-size="12">session cookie</text>
<rect x="265" y="20" width="230" height="170" rx="8" fill="var(--card)" stroke="var(--input)"/>
<text x="380" y="48" text-anchor="middle" font-weight="600">Apps site</text>
<text x="380" y="70" text-anchor="middle" fill="var(--muted-foreground)" font-size="12">example-apps.com</text>
<rect x="290" y="90" width="180" height="36" rx="5" fill="var(--muted)"/>
<text x="380" y="113" text-anchor="middle" font-size="12">web.example-apps.com</text>
<rect x="290" y="136" width="180" height="36" rx="5" fill="var(--muted)"/>
<text x="380" y="159" text-anchor="middle" font-size="12">api.example-apps.com</text>
<rect x="520" y="20" width="230" height="170" rx="8" fill="var(--card)" stroke="var(--input)" stroke-dasharray="5 4"/>
<text x="635" y="48" text-anchor="middle" font-weight="600">Previews site</text>
<text x="635" y="70" text-anchor="middle" fill="var(--muted-foreground)" font-size="12">example-previews.com</text>
<rect x="545" y="90" width="180" height="36" rx="5" fill="var(--muted)"/>
<text x="635" y="113" text-anchor="middle" font-size="12">3f9a…-web.…</text>
<rect x="545" y="136" width="180" height="36" rx="5" fill="var(--muted)"/>
<text x="635" y="159" text-anchor="middle" font-size="12">pull-request code</text>
<g stroke="var(--destructive)" stroke-width="2">
<path d="M247 95 L258 106 M258 95 L247 106"/>
<path d="M502 95 L513 106 M513 95 L502 106"/>
</g>
<text x="380" y="225" text-anchor="middle" fill="var(--muted-foreground)" font-size="12">Cross-site to each other: no shared cookies, and the dashboard's CSRF checks refuse them.</text>
</g>
</svg>
<figcaption>Each kind of domain is its own registrable domain, so code on one can't borrow the others' cookies or same-site trust.</figcaption>
</figure>

`sslip.io` and `nip.io` are public DNS services: a name such as `203-0-113-7.sslip.io` resolves to `203.0.113.7`. They are different sites, which is why the installer uses one for the dashboard and the other for apps. They let HTTPS work right after installation, before you have DNS of your own. For production, use your own domains.

Change the domains in **Settings**, or on the server:

```sh
sudo tond settings set-dashboard-domain tond.example.com
sudo tond settings set-apps-domain example-apps.com
sudo tond settings set-preview-domain example-previews.com
```

Pass an empty string to clear one. tond-proxy picks up a change without a restart.

## Dashboard domain

The dashboard and the API are served only on the dashboard domain, and only over HTTPS. Without a dashboard domain, you can reach the dashboard only through an SSH tunnel.

DNS: an `A` record (and `AAAA` if you use IPv6) for the dashboard domain, pointing at the server.

## Apps domain

When an app without a domain deploys for the first time, Tond gives it the hostname `<app>.<apps domain>` as its primary domain. Background workers get no hostname. If no apps domain is set, the deploy log says the app has no public address yet, and the app needs a domain of your own.

DNS: a wildcard record `*.<apps domain>` pointing at the server, so every app is covered by one record.

## App domains

An app can answer on any number of domains. Each one has a kind:

| Kind | What it does |
| --- | --- |
| Primary | The app's main address. An app has at most one. |
| Alias | Serves the same app as the primary. |
| Redirect | tond-proxy answers with a `308` (the default, which keeps the method) or a `301` to another domain of the same app. The path and query are kept, and the app never sees the request. |

The first serving domain you add becomes the primary, later ones become aliases. If you remove the primary, the oldest alias becomes the primary. The automatic `<app>.<apps domain>` hostname is an ordinary entry in this list, marked **Automatic**.

Manage the list in the app's **Settings → Domain**. **Add domain** asks for the name and whether it **Serves the app** or **Redirects to another domain**. For a serving domain such as `example.com`, you can also tick **Also add `www.example.com`, redirecting here** (or the reverse for a `www.` name), so visitors who type either name end up on one address. Each row shows the domain's DNS and HTTPS status, with **Make primary**, **Redirect** (or **Edit redirect** and **Serve instead**) and a remove button. Select a domain to see the DNS records to create, what it resolves to now, and the certificate that serves it.

With the CLI:

```sh
tond apps domains list web
tond apps domains add web www.example.com       # an alias, or the primary if the app has none
tond apps domains add web example.com --www     # example.com serves, www.example.com redirects to it
tond apps domains add web old.example.com --redirect-to example.com --code 301
tond apps domains set-primary web example.com
tond apps domains redirect web www.example.com --to example.com
tond apps domains remove web old.example.com
```

`tond apps domain web example.com` is a shortcut: it adds the domain if needed and makes it the primary. The previous primary stays as an alias.

A domain must:

- be a valid hostname with at least two labels;
- not be the dashboard domain;
- not be under the preview domain, which serves only previews;
- not be used by another app.

Redirects follow a few rules, so they never chain or point away from the app:

- A redirect points at a serving domain (the primary or an alias) of the same app, never at another redirect.
- A domain that other domains redirect to can't be removed or turned into a redirect until they point elsewhere.
- The primary can't become a redirect. Make another domain primary first.

A domain may be on the same site as the dashboard: you own both. The dashboard then shows a warning, because the app could send same-site requests to the dashboard. For code you don't fully trust, use a domain on a different site.

DNS: an `A` record (and `AAAA` for IPv6) for each domain, pointing at the server. A domain under the apps domain is covered by the wildcard record. If a CDN or another proxy sits in front, it must pass HTTP requests on port 80 through to the server for certificate validation, unless a [DNS provider](#dns-providers-and-wildcard-certificates) covers the domain.

## HTTP and HTTPS

- **Port 443** serves every routed hostname over TLS. Redirect domains are answered there with their `301` or `308`.
- **Port 80** answers Let's Encrypt validation requests. It sends a request for a redirect domain straight to its target over HTTPS, redirects every other request for a hostname Tond serves to HTTPS with `301`, and answers `404` for any other hostname, so it is never an open redirect.

Tond routes an app's domains only while the app is running and serves traffic (not a background worker). A stopped app's domains, redirects included, aren't routed.

## Certificates

tond-proxy requests a certificate for every hostname it routes: the dashboard domain, running apps' domains and running previews. By default it gets one certificate per hostname from Let's Encrypt, validated with the ACME HTTP-01 challenge. A [DNS provider](#dns-providers-and-wildcard-certificates) adds wildcard certificates and DNS-01 validation, and you can [upload a certificate](#uploaded-certificates) instead.

For each TLS connection, tond-proxy picks the certificate for the exact hostname, or else the wildcard one label up (`*.example.com` for `web.example.com`).

![The Certificates page: every hostname, its certificate status and expiry](/img/domains/certificates.light.webp)

| Step | Behaviour |
| --- | --- |
| First request | Soon after the hostname starts being routed |
| While waiting | TLS still works with a self-signed fallback certificate, so browsers show a warning |
| Renewal | When a certificate expires within 30 days |
| Failure | Retried after 1 minute, then with doubling delays up to every 6 hours |
| DNS pre-check | Before ordering, Tond resolves the domain. If it clearly points only at other public addresses, the order is skipped and recorded as a failure with a hint. After the backoff reaches 6 hours, Tond orders anyway, for domains behind a CDN. |

Certificates and keys live under `<data>/certs/<domain>/`, and the ACME account in `<data>/acme_account.json`.

### DNS providers and wildcard certificates

A DNS provider gives Tond API access to your DNS, so it can prove it controls a domain with an `_acme-challenge` TXT record (ACME DNS-01). Add one under **Integrations → DNS providers**:

| Provider | Credential |
| --- | --- |
| Cloudflare | An API token with Zone → DNS → Edit, limited to your zone |
| Hetzner DNS | An API token with read and write permission |
| DigitalOcean | A personal access token with the `domain` create, read and delete scopes |
| Amazon Route 53 | The access key ID and secret access key of an IAM user allowed to change the zone's records |

Each provider lists the zones it hosts, for example `example.com`. **Test** checks every zone against the provider's API. Tond only adds and removes `_acme-challenge` records. Tokens are encrypted at rest and never shown again. Only tond-server holds them: tond-proxy, which faces the internet, asks tond-server over its control socket to publish a record, and tond-server publishes records only for domains Tond serves.

When a provider covers the zone:

- **Wildcard certificates.** tond-proxy orders one certificate for `*.<apps domain>` and one for `*.<preview domain>`, instead of one per app or preview. A new app or preview under them has a trusted certificate at once, without an order of its own, and per-host certificates the wildcard replaces are dropped. If the wildcard fails twice, the hosts fall back to certificates of their own until it is issued.
- **DNS-01 for other domains.** Any other domain in a covered zone, such as a custom app domain or the dashboard domain, is validated with DNS-01 instead of HTTP-01. This also works for domains behind a CDN or not reachable on port 80.

After publishing a record, tond-server waits for it to propagate before the CA checks it: 30 seconds, or 60 for Route 53 (see `TOND_DNS_PROPAGATION_SECS` in [Configuration](/reference/configuration)). The **DNS providers** page shows which wildcard certificates each provider covers.

### Uploaded certificates

To serve a certificate from another CA, click **Upload certificate** on the **Certificates** page (admins, `settings:write`). Enter the domain (a hostname, or a wildcard such as `*.example.com`), the certificate chain in PEM with the leaf first, and its private key. tond-proxy checks that the key matches the certificate, the chain is in order, the certificate is currently valid and covers the domain. It then stores it like its own certificates (key mode `0600`) and serves it.

An uploaded certificate replaces the Let's Encrypt one for that domain, and hosts it covers get no certificate of their own. Tond never renews it: upload the renewed certificate with **Replace** before it expires. **Remove** deletes it, and Tond requests a Let's Encrypt certificate again.

### The Certificates page

**Certificates** in the dashboard lists every certificate with what it serves (the dashboard, an app, a preview, a wildcard for every app or every preview with the number of hosts it covers, or nothing any more), its issuer (**Uploaded** for your own), whether it was validated with DNS-01, days left and state:

| Label | Meaning |
| --- | --- |
| Valid | Issued and not close to expiry |
| Requesting | The first request is in progress |
| Waiting for DNS | The last attempt failed and a retry is scheduled |
| Failed | Issuance failed |
| Expiring soon | Renewal is overdue, or an uploaded certificate is close to expiry |
| Renewal failing | A valid certificate exists, but the last renewal attempt failed |
| Expired | The certificate has expired |
| Untrusted (staging) | Issued by the Let's Encrypt staging CA, which browsers don't trust |

Each problem shows the last error. **Retry now** skips the backoff and tries again at once. Adding a domain to an app also resets its retry state. Hosts that an issued wildcard or uploaded certificate covers aren't listed as waiting for a certificate.

To be told before a certificate expires, uploaded ones included, create a **Certificate expiry** alert rule (default: 14 days). See [Metrics, uptime and alerts](/concepts/observability). For certificate problems, see [Troubleshooting](/operations/troubleshooting).

## Related

- [Previews](/concepts/previews)
- [Apps](/concepts/apps)
- [Configuration](/reference/configuration)
- [CLI reference](/reference/cli)
