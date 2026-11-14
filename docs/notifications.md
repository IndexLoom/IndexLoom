# External notifications

Owners and the legacy administrator token can manage **Settings → Notifications**.
No external notifications are sent until a channel is created and enabled. Email
verification, password recovery, and SMTP settings continue to use the separate
Email section. Notification configuration is stored in the same SQLite database
and uses the existing master-key vault; no new environment variables are required.

## Channels

| Provider | Public fields | Encrypted fields |
| --- | --- | --- |
| Telegram | `chatId`, optional `threadId`, `disableNotification`, `apiBase` | `token` |
| Discord | — | webhook `url` |
| Slack | — | webhook `url` |
| Webhook | — | `url`, optional `bearer` |
| ntfy | `topic`, optional `apiBase`, `priority` (1–5, default 3) | optional `token` |
| Gotify | `apiBase`, `priority` (0–10, default 5) | application `token` |
| Apprise API | — | base `url`, configuration `key` |
| Pushover | `priority` (−2–1, default 0) | application `token`, `user` key |

Telegram defaults to `https://api.telegram.org`. Select **Self-hosted Bot API**
and enter the base URL, such as `http://127.0.0.1:8081`, without `/botTOKEN` or
`/sendMessage`. ntfy defaults to `https://ntfy.sh`; Gotify requires a base URL.
Apprise's encrypted base URL must exclude `/notify/KEY/`, which the adapter appends.
Base URL path prefixes (reverse proxy installations) are preserved. Webhook and
Discord/Slack URLs are complete endpoints, including any credential query string.
The generic webhook POST contains `title`, `body`, `event`, and RFC 3339 `time`.

HTTPS with valid certificates is required by default. `allowInsecureLocal: true`
permits HTTP only for `localhost`, literal loopback addresses, or literal private
IPv4/IPv6 addresses. Use a private IP for LAN services; arbitrary HTTP DNS names
are rejected. URL userinfo and fragments are rejected; public API bases also
reject query strings. Redirects and environment HTTP proxies are disabled. Public
API base URLs must not contain credentials; credential-bearing endpoint URLs
belong in the encrypted `secrets` object. Each request has a five-second connect
and fifteen-second total timeout. At most four queued requests run concurrently.

Secret values are never returned. Channel responses include only `storedSecrets`
(field names). On edit, omit a secret key to preserve it, supply a string to replace
it, or supply JSON `null` to remove it. Blank strings are rejected. Required
credentials can be cleared on a disabled channel. Changing providers discards the
old provider's secrets. Secret fields are limited to 2,048 bytes each, channel
names to 120 bytes, and the database to 100 channels. Updates require the returned
`revision` to avoid lost edits. Configuration changes cancel pending/retrying
messages; a claimed delivery checks its channel revision before sending. An HTTP
request already in flight cannot be revoked by an edit or deletion.

## Events and templates

- `system.startup`: notification worker starts (daemon and native local runtime).
- `auth.login`: successful local, TOTP, or OIDC login. No username, email, tokens,
  verification links, or password-reset links are included.
- `automation.completed` / `automation.failed`: a claimed scheduler run finishes;
  includes the job ID, never upstream error text.
- `indexer.test.passed` / `indexer.test.failed`: asynchronous indexer test result;
  includes the indexer ID, never the indexer's response or credentials.

Each channel explicitly selects its events. Global templates fall back to
`{{title}}` and `{{message}}`. A channel/event override takes precedence; deleting
it restores the global template. Deleting a global template restores the defaults.
Templates support exactly literal `{{title}}`, `{{message}}`, `{{event}}`,
`{{time}}`, `{{app}}`, `{{indexer}}`, and `{{job}}` substitutions. Unknown placeholders
are retained; replacement values are not evaluated again. There is no expression
engine or HTML renderer. Telegram has no parse mode, Slack disables mrkdwn and
unfurling, Discord disables mentions, and Apprise requests text formatting.

Titles are limited to 500 UTF-8 bytes and bodies to 4,000 bytes in template source.
Rendered titles are capped at 250 UTF-16 code units and bodies at 4,000; adapters
apply their narrower limits without splitting UTF-8 sequences or surrogate pairs.
ntfy bodies are also capped at 4,096 UTF-8 bytes. For example,
Discord combines title/body into at most 2,000 characters, Pushover caps the body
at 1,024, and Telegram caps combined text at 4,096. Preview uses sample job/indexer
IDs and React displays its output as text.

## Delivery guarantees and logs

Event producers enqueue in SQLite without doing network I/O. A unique
`(channel_id,event_key)` prevents duplicate enqueue while a delivery record exists.
The worker leases each row for 60 seconds and recovers expired leases after crashes.
HTTP 429, 5xx, and network failures retry with delays of 30, 60, 120, and 240 seconds,
up to five total attempts. Most other HTTP/API rejections fail immediately.
Telegram `ok=false`, Gotify error objects, Pushover `status!=1`, and Slack bodies
other than `ok` are failures even after HTTP 2xx. Recognized Telegram/Gotify 429
and 5xx JSON error codes are retryable. Redirects fail without following Location.

Delivery is **at least once**, not exactly once: a provider may receive a request
before a connection timeout or process crash. Providers do not all support an
idempotency key. Event recording is best effort if SQLite is unavailable or the
10,000 outstanding-delivery cap is reached; this does not fail the originating
login/job/test. Duplicate enqueue protection lasts for the log's retention period.

History contains channel ID, event, status, attempt count, timestamps, duration,
and a fixed safe failure category. Neither request payloads nor provider response
bodies, errors, URLs, or tokens are exposed in history. Terminal payloads are
cleared immediately. Maintenance runs on the first worker tick and approximately
every minute, keeping at most 2,000 terminal records and at most 30 days of history.
Outstanding rows are retained for processing. Synchronous test sends are logged,
never retried, and have a per-channel 15-second cooldown. A successful test means
the provider accepted the request; it cannot confirm human receipt or reading.

## API

All paths below are relative to `/api/v1/notifications` and require **owner** access
(including GET/HEAD). Responses use camelCase. Notification API responses are
`Cache-Control: no-store, private` and validation errors do not echo input values.
External/redirect mode preserves notification paths with the existing routing.

| Method | Path | Operation |
| --- | --- | --- |
| GET / POST | `/channels` | List safe channel metadata / create |
| PUT / DELETE | `/channels/{id}` | Replace public configuration / delete |
| POST | `/channels/{id}/test` | Send immediately; return `success`, `status`, `summary`, `durationMs` |
| GET | `/events` | Event definitions |
| GET | `/templates` | Effective global defaults and stored channel overrides |
| PUT / DELETE | `/templates/{event}?channelId={id}` | Save / reset (omit `channelId` for global) |
| POST | `/preview` | Render `{event,title,body}` without sending |
| GET | `/deliveries?limit=100&before={deliveryId}` | Descending cursor pagination, capped at 200 per page |

Example create payload (dummy credentials only):

```json
{
  "name": "Operations",
  "provider": "telegram",
  "enabled": true,
  "config": { "chatId": "-100123", "disableNotification": false },
  "events": ["automation.failed", "indexer.test.failed"],
  "secrets": { "token": "123:replace-with-your-token" }
}
```

To add a provider, add its credential allowlist and configuration validation in
`notifications/model.rs`, request/result handling in `notifications/providers.rs`,
and matching UI fields in `web/src/lib/notifications.ts` and the settings editor.
Use local HTTP fixtures for adapter tests; do not put live credentials in tests.

Provider contracts: [Telegram](https://core.telegram.org/bots/api#sendmessage),
[Discord](https://docs.discord.com/developers/resources/webhook#execute-webhook),
[Slack](https://docs.slack.dev/messaging/sending-messages-using-incoming-webhooks/),
[ntfy](https://docs.ntfy.sh/publish/), [Gotify](https://gotify.net/docs/pushmsg),
[Apprise API](https://github.com/caronc/apprise-api), [Pushover](https://pushover.net/api).

## Verification

```sh
cargo test -p indexloom-server -p indexloom-storage
cargo fmt --all -- --check
cargo clippy -p indexloom-server -p indexloom-storage --all-targets -- -D warnings
cd web
npm run typecheck
npm run build
npm run i18n:check
```

The optional `npm run test:notifications` browser flow expects a local preview at
`http://127.0.0.1:4175` (override `INDEXLOOM_PREVIEW_URL`) and an existing Playwright
installation (override `INDEXLOOM_PLAYWRIGHT_ENTRY`) with Chromium (optionally
`INDEXLOOM_CHROMIUM_PATH`). It mocks API responses; Rust tests perform actual
loopback HTTP sends and exercise durable worker behavior. No production service
or third-party credentials are needed.
