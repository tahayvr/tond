# Domains and certificates

Tond serves everything over HTTPS through tond-proxy, on ports 80 and 443 of your server. Each hostname gets a free certificate from Let's Encrypt, obtained and renewed automatically. This page explains the kinds of domains Tond uses, what DNS they need, and how certificates are issued.

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

When an app without a domain deploys for the first time, Tond gives it the hostname `<app>.<apps domain>`. Background workers get no hostname. If no apps domain is set, the deploy log says the app has no public address yet, and the app needs a custom domain.

DNS: a wildcard record `*.<apps domain>` pointing at the server, so every app is covered by one record.

## Custom domains

Each app has one domain. It is either the automatic hostname or a custom domain you set. Set it in the app's **Settings → Domain**, or with the CLI:

```sh
tond apps domain web www.example.com
```

A custom domain must:

- be a valid hostname with at least two labels;
- not be the dashboard domain;
- not be under the preview domain, which serves only previews;
- not be used by another app.

A custom domain may be on the same site as the dashboard: you own both. The dashboard then shows a warning, because the app could send same-site requests to the dashboard. For code you don't fully trust, use a domain on a different site.

The **Domain** section checks DNS as you type. It resolves the name, compares it with the server's public addresses, and tells you which records to create. For a domain under the apps domain, it also suggests the wildcard record.

DNS: an `A` record (and `AAAA` for IPv6) pointing at the server. If a CDN or another proxy sits in front, it must pass HTTP requests on port 80 through to the server, for certificate validation.

## HTTP and HTTPS

- **Port 443** serves every routed hostname over TLS.
- **Port 80** answers Let's Encrypt validation requests. It redirects every other request for a hostname Tond serves to HTTPS with `301`, and answers `404` for any other hostname, so it is never an open redirect.

Tond routes an app's domain only while the app is running and serves traffic (not a background worker). A stopped app's domain isn't routed.

## Certificates

tond-proxy obtains certificates from Let's Encrypt with the ACME HTTP-01 challenge, one certificate per hostname. There are no wildcard certificates. It requests a certificate for every hostname it routes: the dashboard domain, running apps' domains and running previews.

![The Certificates page: every hostname, its certificate status and expiry](/img/domains/certificates.light.webp)

| Step | Behaviour |
| --- | --- |
| First request | Soon after the hostname starts being routed |
| While waiting | TLS still works with a self-signed fallback certificate, so browsers show a warning |
| Renewal | When a certificate expires within 30 days |
| Failure | Retried after 1 minute, then with doubling delays up to every 6 hours |
| DNS pre-check | Before ordering, Tond resolves the domain. If it clearly points only at other public addresses, the order is skipped and recorded as a failure with a hint. After the backoff reaches 6 hours, Tond orders anyway, for domains behind a CDN. |

Certificates and keys live under `<data>/certs/<domain>/`, and the ACME account in `<data>/acme_account.json`.

### The Certificates page

**Certificates** in the dashboard lists every certificate with what it serves (the dashboard, an app, a preview or nothing any more), its issuer, days left and state:

| Label | Meaning |
| --- | --- |
| Valid | Issued and not close to expiry |
| Requesting | The first request is in progress |
| Waiting for DNS | The last attempt failed and a retry is scheduled |
| Failed | Issuance failed |
| Expiring soon | Renewal is overdue |
| Renewal failing | A valid certificate exists, but the last renewal attempt failed |
| Expired | The certificate has expired |
| Untrusted (staging) | Issued by the Let's Encrypt staging CA, which browsers don't trust |

Each problem shows the last error. **Retry now** skips the backoff and tries again at once. Saving a domain again also resets its retry state.

To be told before a certificate expires, create a **Certificate expiry** alert rule (default: 14 days). See [Metrics, uptime and alerts](/concepts/observability). For certificate problems, see [Troubleshooting](/operations/troubleshooting).

## Related

- [Previews](/concepts/previews)
- [Apps](/concepts/apps)
- [Configuration](/reference/configuration)
- [CLI reference](/reference/cli)
