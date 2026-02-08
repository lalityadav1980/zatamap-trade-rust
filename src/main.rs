mod api;
mod auth;
mod bootstrap;
mod core;
mod dao;
mod db;
mod instruments;
mod kite;

use crate::core::AppError;
use crate::kite::client::KiteClient;
use crate::{core::AppConfig, core::AppState, db::Db};
use std::sync::Arc;

fn usage() -> &'static str {
        r#"Usage:
    cargo run -- server
    cargo run -- profile
    cargo run -- holdings
    cargo run -- autologin <USER_ID> [--debug] [--force]

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
"#
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    dotenvy::dotenv().ok();

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
    };

    bootstrap::initialize_on_startup(&state).await?;

    let app = api::router(state);
    println!("Listening on http://{addr}");
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .map_err(|e| AppError::KiteApi(e.to_string()))?;
    Ok(())
}

async fn run_autologin(user_id: &str, debug: bool, force: bool) -> Result<(), AppError> {
    let config = AppConfig::from_env()?;
    let db = Db::connect(&config.database_url).await?;
    let state = AppState {
        config: Arc::new(config),
        db: Arc::new(db),
    };

    auth::autologin::maybe_autologin(
        &state,
        user_id,
        auth::autologin::AutoLoginOptions { debug, force },
    )
    .await
}
