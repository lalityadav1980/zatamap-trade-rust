# Ticker (Kite WebSocket)

This document describes the DB-backed ticker client implemented in this repo.

## What it does

- Connects to Kite ticker WebSocket (`wss://ws.kite.trade`)
- Subscribes in `FULL` mode
- Decodes incoming binary tick frames
- Filters ticks to a bounded, known token set
- Updates the in-memory tick store (latest tick per token)
- Optionally prints decoded ticks (rate-limited)

## How to run

Ticker (reads `api_key` + `access_token` from Postgres for the user):

```bash
cargo run -- ticker YOUR_USER_ID
```

Single-process end-to-end:

```bash
cargo run -- e2e YOUR_USER_ID
```

Stop conditions:

- If `TICKER_RUN_SECS` is unset/0: runs until Ctrl+C
- If `TICKER_RUN_SECS>0`: auto-exits after that many seconds (smoke test)

## Token selection (what we subscribe to)

For `ticker`/`e2e`, the token list is built from Postgres:

- current-week NIFTY option instruments (nearest weekly expiry; excludes unrelated symbols like VIX)
- plus NIFTY index token `256265`

This keeps in-memory state bounded and matches the Python behavior we mirrored.

## Tick logging (printing)

Tick processing and tick printing are separate:

- The app always processes ticks and updates the store (when connected)
- Printing decoded ticks can be enabled/disabled

CLI flags:

- `--print-ticks` enables tick printing
- `--no-print-ticks` disables tick printing

Env vars:

```dotenv
# Default: 1 (on)
TICK_LOG_FULL=1

# Default: 500
TICK_LOG_INTERVAL_MS=500
```

Notes:

- Printing is rate-limited to avoid log flooding.
- Even with printing disabled, you should still see periodic `ticker stats` logs with `received_tokens=...`.

## Processing model

- Each decoded tick updates an entry in the in-memory store keyed by `instrument_token`.
- Many tick fields are optional (`Option<T>`). It’s normal to see `Some(...)`/`None` when printing full ticks.
- Derived metrics are designed to be extended (spread/ROC scaffolding exists; greeks can be added later).

## Reconnect behavior

If the WebSocket disconnects or errors, the client reconnects with a backoff. When it reconnects successfully, it re-subscribes and resumes decoding/processing.

## Troubleshooting

- REST preflight / WS 403: access token is expired/invalid for that user → run `autologin` (or complete the callback flow) to refresh it.
- Connected + `received_tokens>0` but no tick lines: tick printing is disabled (`TICK_LOG_FULL=0` or `--no-print-ticks`) or rate-limited by `TICK_LOG_INTERVAL_MS`.
