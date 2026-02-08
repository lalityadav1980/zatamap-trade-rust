use super::error::AppError;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub server_addr: String,
    pub database_url: String,
    pub kite_callback_url: String,
    pub os_type: String,

    // Startup auto-login (initialize_on_startup equivalent)
    pub startup_autologin_user_id: Option<String>,
    pub startup_autologin_os_type: Option<String>,
    pub startup_autologin_debug: bool,
    pub startup_autologin_force: bool,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AppError> {
        let server_addr = std::env::var("SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            let host = std::env::var("PGHOST").unwrap_or_else(|_| "localhost".into());
            let port = std::env::var("PGPORT").unwrap_or_else(|_| "5432".into());
            let db = std::env::var("PGDATABASE").unwrap_or_else(|_| "zatamap_trade".into());
            let user = std::env::var("PGUSER").unwrap_or_else(|_| "zatamap".into());
            let pass = std::env::var("PGPASSWORD").unwrap_or_else(|_| "".into());
            let sslmode = std::env::var("PGSSLMODE").ok();

            // tokio-postgres accepts keyword/value style strings.
            let mut parts = vec![
                format!("host={host}"),
                format!("port={port}"),
                format!("dbname={db}"),
                format!("user={user}"),
            ];
            if !pass.is_empty() {
                parts.push(format!("password={pass}"));
            }
            if let Some(sslmode) = sslmode {
                parts.push(format!("sslmode={sslmode}"));
            }
            parts.join(" ")
        });
        let kite_callback_url = std::env::var("KITE_CALLBACK_URL")
            .map_err(|_| AppError::MissingEnv("KITE_CALLBACK_URL"))?;

        // Used to pick the correct row when trade.profile has multiple entries
        // per user for different OS types.
        //
        // Supported values (recommended): macos | ubuntu
        // Fallback: derived from runtime OS.
        let os_type = std::env::var("OS_TYPE").unwrap_or_else(|_| normalize_os(std::env::consts::OS));

        // Startup autologin controls (mirrors Python initialize_on_startup knobs).
        // If STARTUP_AUTOLOGIN_USER_ID is set, startup will attempt selenium autologin.
        let startup_autologin_user_id = std::env::var("STARTUP_AUTOLOGIN_USER_ID")
            .ok()
            .or_else(|| std::env::var("AUTOLOGIN_USER_ID").ok())
            .filter(|s| !s.trim().is_empty());
        let startup_autologin_os_type = std::env::var("STARTUP_AUTOLOGIN_OS_TYPE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let startup_autologin_debug = parse_bool_env("STARTUP_AUTOLOGIN_DEBUG").unwrap_or(false);
        let startup_autologin_force = parse_bool_env("STARTUP_AUTOLOGIN_FORCE").unwrap_or(false);

        Ok(Self {
            server_addr,
            database_url,
            kite_callback_url,
            os_type,

            startup_autologin_user_id,
            startup_autologin_os_type,
            startup_autologin_debug,
            startup_autologin_force,
        })
    }

    /// Config loader for the `ticker` CLI command.
    ///
    /// Unlike `from_env()`, this does not require `KITE_CALLBACK_URL` because
    /// the websocket ticker connects directly using api_key + access_token.
    pub fn from_env_ticker() -> Result<Self, AppError> {
        let server_addr = std::env::var("SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            let host = std::env::var("PGHOST").unwrap_or_else(|_| "localhost".into());
            let port = std::env::var("PGPORT").unwrap_or_else(|_| "5432".into());
            let db = std::env::var("PGDATABASE").unwrap_or_else(|_| "zatamap_trade".into());
            let user = std::env::var("PGUSER").unwrap_or_else(|_| "zatamap".into());
            let pass = std::env::var("PGPASSWORD").unwrap_or_else(|_| "".into());
            let sslmode = std::env::var("PGSSLMODE").ok();

            let mut parts = vec![
                format!("host={host}"),
                format!("port={port}"),
                format!("dbname={db}"),
                format!("user={user}"),
            ];
            if !pass.is_empty() {
                parts.push(format!("password={pass}"));
            }
            if let Some(sslmode) = sslmode {
                parts.push(format!("sslmode={sslmode}"));
            }
            parts.join(" ")
        });

        let kite_callback_url = std::env::var("KITE_CALLBACK_URL").unwrap_or_default();
        let os_type = std::env::var("OS_TYPE").unwrap_or_else(|_| normalize_os(std::env::consts::OS));

        let startup_autologin_user_id = std::env::var("STARTUP_AUTOLOGIN_USER_ID")
            .ok()
            .or_else(|| std::env::var("AUTOLOGIN_USER_ID").ok())
            .filter(|s| !s.trim().is_empty());
        let startup_autologin_os_type = std::env::var("STARTUP_AUTOLOGIN_OS_TYPE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let startup_autologin_debug = parse_bool_env("STARTUP_AUTOLOGIN_DEBUG").unwrap_or(false);
        let startup_autologin_force = parse_bool_env("STARTUP_AUTOLOGIN_FORCE").unwrap_or(false);

        Ok(Self {
            server_addr,
            database_url,
            kite_callback_url,
            os_type,
            startup_autologin_user_id,
            startup_autologin_os_type,
            startup_autologin_debug,
            startup_autologin_force,
        })
    }
}

fn parse_bool_env(key: &str) -> Option<bool> {
    let v = std::env::var(key).ok()?;
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    Some(matches!(v, "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"))
}

fn normalize_os(runtime: &str) -> String {
    match runtime {
        "macos" => "macos".to_string(),
        "linux" => "ubuntu".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => "unknown".to_string(),
    }
}
