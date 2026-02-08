# zatamap-trade-rust

Minimal Rust API server + CLI helpers for Zerodha Kite Connect.

This repository contains:

- An HTTP server (Axum) that:
	- serves `/api/health`
	- provides a login URL for a user (`/api/kite/login_url`)
	- receives Kite callback and stores tokens in Postgres (`/api/kite/callback`)
- A CLI that can call Kite REST directly (`profile`, `holdings`)
- An optional Selenium-based auto-login flow that can update `trade.profile.access_token`

> Security note: do not commit real secrets (API keys, access tokens, DB passwords).
> Use `.env` locally (already gitignored) and store per-user Kite/Zerodha values in Postgres.

---

## Prerequisites

- Rust toolchain (stable)
- Postgres (local or remote)
- A Zerodha Kite Connect API key + API secret
- (Optional, only for `autologin`) Google Chrome/Chromium + Chromedriver

---

## Install dependencies

### macOS

1) Install Xcode CLT (needed for building some crates):

```bash
xcode-select --install
```

2) Install Rust (recommended: rustup):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
cargo --version
```

3) Install Postgres (Homebrew example):

```bash
brew update
brew install postgresql@16
brew services start postgresql@16
psql --version
```

4) (Optional for Selenium autologin) Install Chrome and a chromedriver:

- Chrome:

```bash
brew install --cask google-chrome
```

- Chromedriver options:
	- Use system chromedriver (if installed):

```bash
brew install --cask chromedriver
chromedriver --version
```

	- Or use the pinned driver already in this repo (Apple Silicon only):

```bash
ls -la .drivers/chromedriver-144/chromedriver-mac-arm64/chromedriver
```

If you use the pinned driver, you will set `CHROMEDRIVER_PATH` to that file.

### Ubuntu

1) Install build tools + curl/git:

```bash
sudo apt update
sudo apt install -y \
	build-essential pkg-config libssl-dev ca-certificates curl git
```

2) Install Rust (rustup):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
cargo --version
```

3) Install Postgres:

```bash
sudo apt install -y postgresql postgresql-contrib
psql --version
```

4) (Optional for Selenium autologin) Install Chrome/Chromium + chromedriver.

Depending on Ubuntu version, packages differ. One common setup is Chromium:

```bash
sudo apt install -y chromium-browser chromium-chromedriver || true
sudo apt install -y chromium chromium-driver || true
chromedriver --version
```

If `chromedriver` is not available via apt on your system, download the matching Chromedriver for your Chrome/Chromium version and set `CHROMEDRIVER_PATH`.

---

## Clone and build

```bash
git clone https://github.com/lalityadav1980/zatamap-trade-rust.git
cd zatamap-trade-rust
cargo build
```

---

## Create `.env`

Copy the template and edit it:

```bash
cp .env.example .env
```

Required env vars for the server:

- `KITE_CALLBACK_URL` (must match what is configured in your Kite developer console)
- `DATABASE_URL` (or `PG*` vars)

Common env vars (server/autologin):

```dotenv
SERVER_ADDR=127.0.0.1:8080

# Either set DATABASE_URL...
DATABASE_URL=host=localhost port=5432 dbname=zatamap_trade user=zatamap password=your_password sslmode=disable

# ...or use PG* vars (AppConfig builds DATABASE_URL if DATABASE_URL is not set)
# PGHOST=localhost
# PGPORT=5432
# PGDATABASE=zatamap_trade
# PGUSER=zatamap
# PGPASSWORD=your_password
# PGSSLMODE=disable

# This is the base callback URL.
# The server will append `userid=<USER_ID>` if you do not include it.
KITE_CALLBACK_URL=http://127.0.0.1:8080/api/kite/callback

# Optional: choose the profile row by OS.
# Defaults to: macos on macOS, ubuntu on Linux.
OS_TYPE=macos

# Optional: some Kite accounts reject redirect_url= in the login request.
# Leave this off unless you need it.
# KITE_INCLUDE_REDIRECT_URL=1
```

CLI-only env vars (for direct REST calls, no DB needed):

```dotenv
# KITE_API_KEY=your_api_key
# KITE_ACCESS_TOKEN=your_access_token
```

---

## Postgres setup

The application expects a schema named `trade` and a table `trade.profile`.

### Create database and schema

Example for local Postgres (adjust usernames/passwords as needed):

```bash
psql postgres
```

```sql
-- Run inside psql
CREATE USER zatamap WITH PASSWORD 'change_me';
CREATE DATABASE zatamap_trade OWNER zatamap;
\c zatamap_trade

CREATE SCHEMA IF NOT EXISTS trade;

-- Minimal table used by this app (columns referenced in src/dao/profile_dao.rs)
CREATE TABLE IF NOT EXISTS trade.profile (
	userid              text        NOT NULL,
	os_type              text        NOT NULL,
	api_key              text        NOT NULL,
	api_secret           text        NOT NULL,
	access_token         text,
	request_token        text,
	public_token         text,
	zerodha_password     text,
	zerodha_pin          text,
	totp_secret          text,
	chrome_binary_path   text,
	chromedriver_path    text,
	updated_at           timestamptz NOT NULL DEFAULT now(),
	PRIMARY KEY (userid, os_type)
);
```

### Seed a user row

Insert your Kite API key/secret for the user id you will use:

```sql
INSERT INTO trade.profile (userid, os_type, api_key, api_secret, updated_at)
VALUES ('YOUR_USER_ID', 'macos', 'YOUR_KITE_API_KEY', 'YOUR_KITE_API_SECRET', now())
ON CONFLICT (userid, os_type)
DO UPDATE SET api_key = EXCLUDED.api_key,
							api_secret = EXCLUDED.api_secret,
							updated_at = now();
```

If you run on Ubuntu, set `os_type` to `ubuntu` (or set `OS_TYPE=ubuntu` in `.env`) and insert a matching row.

---

## Run

### 1) Run API server

```bash
cargo run -- server
```

Health check:

```bash
curl -s http://127.0.0.1:8080/api/health | jq .
```

> Tip: if you don’t have `jq`, install it (`brew install jq` or `sudo apt install jq`) or just run `curl` without piping.

### 2) Kite login URL and callback flow

1) Request a login URL for a user (the server reads `api_key` from `trade.profile` using `userid` + `OS_TYPE`):

```bash
curl -s "http://127.0.0.1:8080/api/kite/login_url?user_id=YOUR_USER_ID" | jq .
```

2) Open the returned `login_url` in a browser and complete the Zerodha login.

3) After login, Kite redirects to your `KITE_CALLBACK_URL` and includes `request_token=...`.
The server exchanges it for an access token and stores `request_token/access_token/public_token` into `trade.profile`.

You can also call callback manually (useful for debugging):

```bash
curl -s "http://127.0.0.1:8080/api/kite/callback?user_id=YOUR_USER_ID&request_token=PASTE_REQUEST_TOKEN" | jq .
```

---

## CLI commands (no DB)

These commands call Kite REST directly, using env vars:

```bash
export KITE_API_KEY="..."
export KITE_ACCESS_TOKEN="..."

cargo run -- profile
cargo run -- holdings
```

REST calls use header: `Authorization: token <api_key>:<access_token>`.

---

## AutoLogin (Selenium)

This mode uses Chromedriver + Chrome to log in and extract a `request_token`, then exchanges it and stores tokens into Postgres.

### Dependencies

- Chrome/Chromium must be installed
- Chromedriver must be running OR spawnable by this app

Chromedriver connection behavior:

- If `CHROMEDRIVER_URL` is set: the app connects to it (it will NOT spawn chromedriver).
- If `CHROMEDRIVER_URL` is NOT set: the app tries to spawn chromedriver.
	Spawn path order:
	1) `CHROMEDRIVER_PATH` (if it exists)
	2) `trade.profile.chromedriver_path`
	3) a pinned driver under `.drivers/` (if present for your OS)
	4) system paths like `/usr/local/bin/chromedriver`

Chrome binary path order:

1) `CHROME_BINARY_PATH`
2) `trade.profile.chrome_binary_path`
3) OS default (macOS: `/Applications/Google Chrome.app/...`, Ubuntu: `/usr/bin/google-chrome`)

### Required DB fields for autologin

Add these fields to your `trade.profile` row for the same `(userid, os_type)`:

- `zerodha_password`
- `zerodha_pin`
- `totp_secret` (base32)

Example update:

```sql
UPDATE trade.profile
SET zerodha_password = 'YOUR_PASSWORD',
		zerodha_pin = 'YOUR_PIN',
		totp_secret = 'YOUR_TOTP_BASE32',
		updated_at = now()
WHERE userid = 'YOUR_USER_ID' AND os_type = 'macos';
```

### Run autologin

```bash
cargo run -- autologin YOUR_USER_ID
```

Useful flags:

- `--force` forces login even if `access_token` already exists
- `--debug` enables extra logging and writes debug artifacts on failure

```bash
cargo run -- autologin YOUR_USER_ID --debug --force
```

Debug artifacts (written in the current working directory on failure):

- `autologin_failure_<USER_ID>_<timestamp>.png`
- `autologin_failure_<USER_ID>_<timestamp>.html`

Selenium-related env vars:

- `CHROMEDRIVER_URL` (default `http://127.0.0.1:9515`)
- `CHROMEDRIVER_PORT` (used only when spawning chromedriver; default `9515`)
- `SELENIUM_HEADLESS` (default `1`; if `--debug` and unset, defaults to headful)
- `CHROMEDRIVER_PATH` (override chromedriver binary to spawn)
- `CHROME_BINARY_PATH` (override Chrome binary)

---

## Troubleshooting

- DB connection errors: confirm `DATABASE_URL` (or `PG*`) and that Postgres is reachable.
- `/api/kite/login_url` returns 404 User not found: insert the `(userid, os_type)` row in `trade.profile`.
- Kite redirect errors about domain mismatch: ensure your Kite app’s Redirect URL matches `KITE_CALLBACK_URL` (including scheme/host/port/path).
- Autologin fails to create WebDriver session: verify `chromedriver --version` matches your installed Chrome/Chromium major version.
