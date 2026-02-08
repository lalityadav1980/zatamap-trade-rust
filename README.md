# zatamap-trade-rust

Minimal Rust CLI for Zerodha Kite Connect REST APIs.

## Prereqs

- Rust toolchain (stable)
- Zerodha Kite Connect API key
- A valid access token (generated via the Kite Connect login flow)

## Setup

1. Copy env file:

```bash
cp .env.example .env
```

2. Fill in `DATABASE_URL` (or `PG*`) and `KITE_CALLBACK_URL` in `.env`.

## Run

From this folder:

### Run API server

```bash
cargo run -- server
```

Endpoints:

- `GET /api/health`
- `GET /api/kite/login_url?user_id=...` (returns login URL; reads `api_key` from `trade.profile`)
- `GET /api/kite/callback?user_id=...&request_token=...` (Kite redirect target; exchanges `request_token` and updates `trade.profile.access_token`)

### Run CLI calls (direct Kite REST)

Set `KITE_API_KEY` and `KITE_ACCESS_TOKEN` in `.env`, then:

```bash
cargo run -- profile
cargo run -- holdings
```

## Notes

- REST calls use header: `Authorization: token <api_key>:<access_token>`.
- The server implements the request-token exchange flow; you still need a browser login to get redirected to `/kite/callback`.

## AutoLogin (Selenium)

Run the Selenium-based login flow that updates `trade.profile.access_token`:

```bash
cargo run -- autologin <USER_ID>
```

Debug mode (writes a screenshot PNG on failure):

```bash
cargo run -- autologin <USER_ID> --debug
```

Selenium-related env vars:

- `CHROMEDRIVER_URL` (optional; default `http://127.0.0.1:9515`)
- `CHROMEDRIVER_PORT` (used only when the app spawns chromedriver; default `9515`)
- `SELENIUM_HEADLESS` (`1` by default; if `--debug` and unset, it defaults to headful)

If `CHROMEDRIVER_URL` is not set, the autologin code will try to spawn chromedriver from `trade.profile.chromedriver_path`.
