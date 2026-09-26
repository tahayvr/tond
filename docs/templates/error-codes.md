# Error codes

Every API error has the same shape and a stable `code` you can switch on. The HTTP status tells you the class of problem, `message` explains it in plain words, and `hint` (when present) says what to do next.

## The error envelope

A successful response:

```json
{
  "success": true,
  "data": { "id": "…", "name": "web" }
}
```

A failed one:

```json
{
  "success": false,
  "data": null,
  "error": {
    "code": "SCOPE_DENIED",
    "message": "This API token doesn't have the apps:deploy scope. Create a token that includes it.",
    "hint": "Use a token that includes the scope, or ask an admin for a role that has it."
  }
}
```

- `code` is always present and never changes meaning. New codes can be added within `/api/v1`, so handle an unknown code by its HTTP status.
- `message` is for people. Its wording can change between releases; don't parse it.
- `hint` is optional advice. `field` names the request field at fault, when there is one (for example `"field": "name"` on an invalid app name).

Failures that happen before a handler runs (a body that isn't valid JSON, a missing field, an unknown path under `/api`, a wrong HTTP method, a body over the size limit) use the same envelope, so every `/api/v1` error can be read the same way.

## Codes

The same table drives the server, the [OpenAPI document](/reference/api/) (`x-tond-codes` on the `ErrorCode` schema) and the CLI's [exit codes](/reference/exit-codes).

<!-- table:error-codes -->

## Retries and idempotency

Send an `Idempotency-Key` header with any `POST`, `PUT`, `PATCH` or `DELETE` under `/api/v1` and the server remembers the request for 24 hours:

- A repeat with the same key and the same body, after the first finished with a 2xx, gets the stored response back (with `Idempotent-Replayed: true`). Nothing runs twice.
- A repeat while the first is still running gets `409 IDEMPOTENCY_KEY_IN_USE`. Retry shortly with the same key.
- The same key with a different body gets `422 IDEMPOTENCY_KEY_REUSED`.

Errors are not stored, so you can fix the cause and retry with the same key. Sign-in, webhooks, terminal tickets and uploads are excluded. The `tond` CLI sends a fresh key with every mutating request; pass `--idempotency-key` (or `TOND_IDEMPOTENCY_KEY`) to choose it, for example in a CI job that may run twice.

## Rate limiting

Every API request over TCP counts against a per-client token bucket: a burst of 60 requests, refilled at 10 per second. Clients are identified by IP address; IPv6 clients are grouped by `/64`. When the bucket is empty the API answers `429 RATE_LIMITED` with a `Retry-After` header (seconds).

Failed sign-ins and 2FA attempts have their own backoff. After 5 failures from one client, or 10 against one account from anywhere, further attempts are refused for 2 seconds, doubling with every failure up to 15 minutes. Failures are forgotten after 15 minutes without a new one. While locked out, even the correct password is refused.

Webhook endpoints have their own per-app limit. The CLI on the root-only Unix socket is never rate limited.

## Same-origin and CSRF checks

Browser requests are checked before they reach a handler. A rejected request gets `403 CSRF_REJECTED` with one of these messages:

| Message | Cause |
| --- | --- |
| `Cross-origin request blocked` | A state-changing request (anything but GET, HEAD, OPTIONS) or a WebSocket upgrade carried an `Origin` header that is neither `https://<dashboard domain>` nor a loopback origin. |
| `Cross-site request blocked` | A cookie-authenticated state-changing request had `Sec-Fetch-Site: cross-site` or `same-site`. |
| `Missing X-Tond-CSRF header` | A cookie-authenticated state-changing request did not send the `X-Tond-CSRF` header. The dashboard sends it on every request. |

Clients that authenticate with a bearer token and send no Tond cookie are not affected by the header rule. A WebSocket upgrade without an `Origin` header (a CLI or a script) has its cookies removed, so it must authenticate with a token or a terminal ticket.

If the dashboard shows these errors, open it on the configured dashboard domain, not on the server's IP address or another name.
