mod api;
mod auth;
mod bootstrap;
mod core;
mod dao;
mod db;
mod instruments;
mod kite;
mod ticks;

use crate::core::AppError;
use crate::kite::client::KiteClient;
use crate::kite::ws::{KiteTickerWs, TickLogConfig};
use crate::ticks::{TickStore, TokenMeta};
use crate::{core::AppConfig, core::AppState, db::Db};
use std::sync::Arc;
use tracing::{info, warn};

fn usage() -> &'static str {
        r#"Usage:
    cargo run -- server
    cargo run -- profile
    cargo run -- holdings
    cargo run -- autologin <USER_ID> [--debug] [--force]
    cargo run -- e2e <USER_ID> [--debug] [--force] [--no-force] [--print-ticks] [--no-print-ticks]
    cargo run -- ticker <USER_ID> [--print-ticks] [--no-print-ticks]

Env (CLI):
    KITE_API_KEY
    KITE_ACCESS_TOKEN

Env (server/autologin):
    SERVER_ADDR (default 127.0.0.1:8080)
    DATABASE_URL  (or PGHOST/PGPORT/PGDATABASE/PGUSER/PGPASSWORD/PGSSLMODE)
    KITE_CALLBACK_URL

Optional:
    AUTOLOGIN_USER_ID (legacy alias for STARTUP_AUTOLOGIN_USER_ID)
    STARTUP_AUTOLOGIN_USER_ID (runs autologin during server startup)
    STARTUP_AUTOLOGIN_OS_TYPE (overrides OS_TYPE just for startup autologin)
    STARTUP_AUTOLOGIN_DEBUG (1/true enables screenshot+HTML dumps on failure)
    STARTUP_AUTOLOGIN_FORCE (1/true forces login even if token exists)
    CHROMEDRIVER_URL (default http://127.0.0.1:9515)
    CHROMEDRIVER_PORT (used only when spawning chromedriver; default 9515)
    SELENIUM_HEADLESS (default 1; if --debug and not set, defaults to 0)
    CHROMEDRIVER_PATH (override chromedriver binary to spawn)
    CHROME_BINARY_PATH (override Chrome binary path)

Ticker logging:
    TICK_LOG_FULL (default 1/on; set to 0/off to disable)
    TICK_LOG_INTERVAL_MS (default 500; rate-limit tick logs)
"#
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    dotenvy::dotenv().ok();
    init_tracing();

    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "server".to_string());

    match cmd.as_str() {
        "server" => run_server().await?,
        "autologin" => {
            let user_id = args.next().unwrap_or_default();
            if user_id.is_empty() {
                eprintln!("Missing USER_ID\n\n{}", usage());
                std::process::exit(2);
            }

            let mut debug = false;
            let mut force = false;
            for a in args {
                match a.as_str() {
                    "--debug" => debug = true,
                    "--force" => force = true,
                    _ => {
                        eprintln!("Unknown flag for autologin: {a}\n\n{}", usage());
                        std::process::exit(2);
                    }
                }
            }

            run_autologin(&user_id, debug, force).await?;
        }
        "e2e" => {
            let user_id = args.next().unwrap_or_default();
            if user_id.is_empty() {
                eprintln!("Missing USER_ID\n\n{}", usage());
                std::process::exit(2);
            }

            // For end-to-end runs we default to force=true so the token is fresh.
            let mut debug = false;
            let mut force = true;
            let mut tick_log_enabled_override: Option<bool> = None;
            for a in args {
                match a.as_str() {
                    "--debug" => debug = true,
                    "--force" => force = true,
                    "--no-force" => force = false,
                    "--print-ticks" => tick_log_enabled_override = Some(true),
                    "--no-print-ticks" => tick_log_enabled_override = Some(false),
                    _ => {
                        eprintln!("Unknown flag for e2e: {a}\n\n{}", usage());
                        std::process::exit(2);
                    }
                }
            }

            run_e2e(&user_id, debug, force, tick_log_enabled_override).await?;
        }
        "ticker" => {
            let user_id = args.next().unwrap_or_default();
            if user_id.is_empty() {
                eprintln!("Missing USER_ID\n\n{}", usage());
                std::process::exit(2);
            }
            let mut tick_log_enabled_override: Option<bool> = None;
            for a in args {
                match a.as_str() {
                    "--print-ticks" => tick_log_enabled_override = Some(true),
                    "--no-print-ticks" => tick_log_enabled_override = Some(false),
                    _ => {
                        eprintln!("Unknown flag for ticker: {a}\n\n{}", usage());
                        std::process::exit(2);
                    }
                }
            }
            run_ticker(&user_id, tick_log_enabled_override).await?;
        }
        "profile" | "holdings" => {
            let api_key =
                std::env::var("KITE_API_KEY").map_err(|_| AppError::MissingEnv("KITE_API_KEY"))?;
            let access_token = std::env::var("KITE_ACCESS_TOKEN")
                .map_err(|_| AppError::MissingEnv("KITE_ACCESS_TOKEN"))?;

            let kite = KiteClient::new(&api_key, &access_token)?;
            if cmd == "profile" {
                let profile = kite.profile().await?;
                println!("{}", serde_json::to_string_pretty(&profile)?);
            } else {
                let holdings = kite.holdings().await?;
                println!("{}", serde_json::to_string_pretty(&holdings)?);
            }
        }
        _ => {
            eprintln!("Unknown command: {}\n\n{}", cmd, usage());
            std::process::exit(2);
        }
    }

    Ok(())
}

async fn run_server() -> Result<(), AppError> {
    let config = AppConfig::from_env()?;
    let db = Db::connect(&config.database_url).await?;

    let addr: std::net::SocketAddr = config
        .server_addr
        .parse()
        .map_err(|e| AppError::KiteApi(format!("Invalid SERVER_ADDR: {e}")))?;

    let state = AppState {
        config: Arc::new(config),
        db: Arc::new(db),
        ticks: Arc::new(TickStore::default()),
    };

    bootstrap::initialize_on_startup(&state).await?;

    let app = api::router(state);
    info!(addr = %addr, "server listening");
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .map_err(|e| AppError::KiteApi(e.to_string()))?;
    Ok(())
}

fn init_tracing() {
    // HFT-friendly logging defaults:
    // - Off by default for noisy modules via RUST_LOG
    // - Optional JSON logs for ingestion
    //
    // Examples:
    // - RUST_LOG=info
    // - RUST_LOG=zatamap_trade_rust=debug,tower_http=info
    // - LOG_FORMAT=json

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let json = std::env::var("LOG_FORMAT")
        .ok()
        .map(|v| v.trim().eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    if json {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .compact()
            .init();
    }

    warn!("tracing initialized");
}

async fn run_autologin(user_id: &str, debug: bool, force: bool) -> Result<(), AppError> {
    let config = AppConfig::from_env()?;
    let db = Db::connect(&config.database_url).await?;
    let state = AppState {
        config: Arc::new(config),
        db: Arc::new(db),
        ticks: Arc::new(TickStore::default()),
    };

    auth::autologin::maybe_autologin(
        &state,
        user_id,
        auth::autologin::AutoLoginOptions { debug, force },
    )
    .await
}

async fn run_e2e(
    user_id: &str,
    debug: bool,
    force: bool,
    tick_log_enabled_override: Option<bool>,
) -> Result<(), AppError> {
    run_autologin(user_id, debug, force).await?;
    run_ticker(user_id, tick_log_enabled_override).await
}

async fn run_ticker(user_id: &str, tick_log_enabled_override: Option<bool>) -> Result<(), AppError> {
    let config = AppConfig::from_env_ticker()?;
    let db = Db::connect(&config.database_url).await?;
    let state = AppState {
        config: Arc::new(config),
        db: Arc::new(db),
        ticks: Arc::new(TickStore::default()),
    };

    let os_type = state.config.os_type.clone();

    let creds = match dao::profile_dao::get_user_kite_creds_for_os(&state.db, user_id, &os_type)
        .await?
    {
        Some(c) => Some(c),
        None => dao::profile_dao::get_user_kite_creds(&state.db, user_id).await?,
    };
    let creds = creds.ok_or_else(|| AppError::KiteApi(format!("user not found in trade.profile: {user_id}")))?;
    let access_token = creds
        .access_token
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::KiteApi(format!("no access_token for user_id={user_id} (run autologin first)")))?;

    let at_len = access_token.len();
    let at_tail = access_token.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>();
    info!(user_id = user_id, access_token_len = at_len, access_token_tail4 = %at_tail, "loaded access_token from DB");

    // Preflight: verify token works for REST. If this fails, WS will also fail.
    let kite = KiteClient::new(&creds.api_key, &access_token)?;
    match kite.profile().await {
        Ok(_) => info!(user_id = user_id, "kite REST auth preflight OK"),
        Err(e) => {
            warn!(user_id = user_id, error = %e, "kite REST auth preflight failed (token likely expired; run autologin)");
        }
    }

    // Select NIFTY current-week option tokens from DB.
    // This mirrors the Python flow which only subscribes to the nearest weekly expiry.
    let (_expiry, rows) = dao::instrument_dao::fetch_nifty_current_week_option_meta(&state.db, 7).await?;
    let mut metas: Vec<TokenMeta> = rows
        .into_iter()
        .map(|r| {
            TokenMeta::new(
                r.instrument_token,
                r.tradingsymbol,
                r.instrument_type,
                r.expiry,
                r.strike,
            )
        })
        .collect();

    // Always include NIFTY index token.
    const NIFTY_INDEX_TOKEN: i32 = 256265;
    metas.push(TokenMeta::new(
        NIFTY_INDEX_TOKEN,
        "NIFTY",
        "INDEX",
        Option::<String>::None,
        None,
    ));

    let sample: Vec<(i32, String)> = metas
        .iter()
        .take(20)
        .map(|m| (m.instrument_token, m.tradingsymbol.to_string()))
        .collect();
    info!(sample = ?sample, "token→tradingsymbol sample");

    // Seed store with token→tradingsymbol mapping and future option math inputs.
    state.ticks.seed_meta(metas.clone());

    let mut tokens: Vec<i32> = metas.iter().map(|m| m.instrument_token).collect();
    tokens.sort_unstable();
    tokens.dedup();

    info!(user_id = user_id, os_type = %os_type, tokens = tokens.len(), "starting kite ticker ws");

    let mut log = TickLogConfig::from_env();
    let has_override = tick_log_enabled_override.is_some();
    if let Some(v) = tick_log_enabled_override {
        log.enabled = v;
    }

    info!(
        tick_log_enabled = log.enabled,
        tick_log_interval_ms = log.interval.as_millis() as u64,
        tick_log_overridden = has_override,
        "ticker tick-log config"
    );
    let ws = KiteTickerWs::new(creds.api_key, access_token, tokens, state.ticks.clone(), log);
    let handle = ws.spawn();

    // Periodic health logs (does not log individual ticks to avoid flooding).
    let store = state.ticks.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            info!(
                subscribed_tokens = store.len(),
                received_tokens = store.received_token_count(),
                "ticker stats"
            );
        }
    });

    let run_secs: Option<u64> = std::env::var("TICKER_RUN_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0);

    match run_secs {
        Some(secs) => {
            info!(secs = secs, "ticker will auto-exit (smoke test)");
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            info!("ticker timer elapsed; stopping");
        }
        None => {
            tokio::signal::ctrl_c()
                .await
                .map_err(|e| AppError::KiteApi(format!("ctrl-c handler failed: {e}")))?;
            info!("ctrl-c received; stopping");
        }
    }
    handle.abort();
    Ok(())
}
