use crate::{
    core::{AppError, AppState},
    dao::profile_dao,
    kite::auth,
};
use super::selenium::{self, WebDriver};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::process::Child;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Default)]
pub struct AutoLoginOptions {
    pub debug: bool,
    pub force: bool,
}

/// Placeholder for the Python `initialize_on_startup` Selenium auto-login.
///
/// Implementing a full Selenium flow in Rust typically uses a WebDriver client
/// (e.g. talk to chromedriver) and environment-provided credentials/2FA.
/// This project keeps the framework hook here so you can plug that in.
pub async fn maybe_autologin(
    state: &AppState,
    user_id: &str,
    options: AutoLoginOptions,
) -> Result<(), AppError> {
    maybe_autologin_for_os(state, user_id, &state.config.os_type, options).await
}

pub async fn maybe_autologin_for_os(
    state: &AppState,
    user_id: &str,
    os_type: &str,
    options: AutoLoginOptions,
) -> Result<(), AppError> {
    let login = profile_dao::get_user_zerodha_login_for_os(&state.db, user_id, os_type).await?;
    let Some(login) = login else {
        println!("AutoLogin: user not found in trade.profile: {user_id}");
        return Ok(());
    };

    let has_token = login
        .access_token
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_token && !options.force {
        println!("AutoLogin: existing access_token present for user={user_id}");
        return Ok(());
    }
    if has_token && options.force {
        println!("AutoLogin: forcing re-login even though access_token exists for user={user_id}");
    }

    let chromedriver_url_env = std::env::var("CHROMEDRIVER_URL").ok();
    let chromedriver_url_from_env = chromedriver_url_env.is_some();
    let chromedriver_port: u16 = std::env::var("CHROMEDRIVER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(9515);

    let chromedriver_path_override = std::env::var("CHROMEDRIVER_PATH")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let chrome_binary_path_override = std::env::var("CHROME_BINARY_PATH")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let headless = match std::env::var("SELENIUM_HEADLESS") {
        Ok(v) => v != "0",
        Err(_) => !options.debug,
    };

    let effective_os = login
        .os_type
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(os_type);

    let redirect_url = auth::callback_url_for_user(&state.config.kite_callback_url, user_id);
    // This mirrors your Python function `login_and_get_tokens`.
    // Important: we pass the per-user redirect_url into Kite login so the final redirect contains it.
    let kite_connect_login_url = auth::login_url(&login.api_key, &redirect_url);
    println!("AutoLogin: starting selenium flow for user={user_id}");
    println!("AutoLogin: os_type={effective_os}");
    println!("AutoLogin: headless={headless} debug={}", options.debug);
    println!("AutoLogin: redirect_url={redirect_url}");

    let mut spawned: Option<Child> = None;
    let chromedriver_url = chromedriver_url_env
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{chromedriver_port}"));

    let selenium_options = selenium::SeleniumOptions {
        headless,
        chrome_binary_path: chrome_binary_path_override
            .clone()
            .or_else(|| login.chrome_binary_path.clone())
            .or_else(|| default_chrome_binary(effective_os).map(|s| s.to_string())),
    };

    let driver = match WebDriver::connect_with_options(&chromedriver_url, selenium_options.clone()).await {
        Ok(d) => d,
        Err(e) => {
            // Best-effort: if chromedriver isn't already running and we have a per-user path, spawn it.
            if !chromedriver_url_from_env {
                let spawn_path = choose_chromedriver_spawn_path(
                    chromedriver_path_override.clone(),
                    login.chromedriver_path.clone(),
                    effective_os,
                );

                if let Some(path) = spawn_path.as_deref() {
                    println!("AutoLogin: chromedriver not reachable; spawning chromedriver='{path}'");
                    spawned = Some(spawn_chromedriver(path, chromedriver_port)?);
                    tokio::time::sleep(Duration::from_millis(700)).await;
                    WebDriver::connect_with_options(&chromedriver_url, selenium_options.clone()).await?
                } else {
                    return Err(e);
                }
            } else {
                return Err(e);
            }
        }
    };

    println!("AutoLogin: chromedriver={chromedriver_url}");
    let password = login
        .zerodha_password
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::KiteApi("Missing zerodha_password in trade.profile".to_string()))?;
    let pin = login
        .zerodha_pin
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::KiteApi("Missing zerodha_pin in trade.profile".to_string()))?;

    let result = run_login_flow(
        &driver,
        &kite_connect_login_url,
        user_id,
        password,
        pin,
        login.totp_secret.as_deref(),
        &redirect_url,
        options,
    )
    .await;

    // Always quit the browser session.
    let _ = driver.quit().await;
    if let Some(mut child) = spawned {
        let _ = child.kill();
        let _ = child.wait();
    }
    let request_token = result?;

    let session = auth::exchange_request_token(&login.api_key, &login.api_secret, &request_token).await?;
    let updated = profile_dao::update_session_tokens_for_os(
        &state.db,
        user_id,
        os_type,
        &request_token,
        &session.access_token,
        session.public_token.as_deref(),
    )
    .await?;
    if updated == 0 {
        return Err(AppError::KiteApi("User not found while updating token".to_string()));
    }

    println!("AutoLogin: updated access_token in DB for user={user_id}");

    // ------------------------------------------------------------ housekeeping
    // Mirror the Python flow: after successful login, refresh instruments into Postgres.
    let kite = crate::kite::client::KiteClient::new(&login.api_key, &session.access_token)?;
    let n = crate::instruments::refresh_trade_instruments(&state.db, &kite).await?;
    println!("AutoLogin: refreshed trade.instrument rows={n} for user={user_id}");
    Ok(())
}
async fn run_login_flow(
    driver: &WebDriver,
    login_url: &str,
    user_id: &str,
    password: &str,
    pin: &str,
    totp_secret: Option<&str>,
    redirect_url: &str,
    options: AutoLoginOptions,
) -> Result<String, AppError> {
    let r = async {
        driver.goto(login_url).await?;

        // 1) Enter user ID
        let user_id_input = driver
            .wait_for_any_css(
                &[
                    "#userid",
                    "#user_id",
                    "input[name='user_id']",
                    "input[name='userid']",
                    "input[autocomplete='username']",
                    "input[type='text']",
                ],
                Duration::from_secs(30),
            )
            .await?;
        safe_send_keys(driver, &user_id_input, user_id).await?;

        // 2) Enter password
        let password_input = driver
            .wait_for_any_css(
                &[
                    "#password",
                    "input[name='password']",
                    "input[type='password']",
                ],
                Duration::from_secs(30),
            )
            .await?;
        safe_send_keys(driver, &password_input, password).await?;

        // 3) Click login (submit)
        let login_button = driver.find_xpath("//button[@type='submit']").await?;
        driver.click(&login_button).await?;

        // 4) PIN input
        tokio::time::sleep(Duration::from_secs(2)).await;
        let pin_input = driver
            .wait_for_css(".login .twofa-form .twofa-value.number input", Duration::from_secs(30))
            .await?;
        safe_send_keys(driver, &pin_input, pin).await?;

        // 5) Continue (submit)
        let cont = driver.find_xpath("//button[@type='submit']").await?;
        driver.click(&cont).await?;

        // 6) Optional TOTP
        tokio::time::sleep(Duration::from_secs(1)).await;
        let otp_input = driver.find_css("input[label='External TOTP']").await.ok();
        if let (Some(el), Some(secret)) = (otp_input, totp_secret) {
            let code = generate_totp(secret)?;
            safe_send_keys(driver, &el, &code).await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        // 6.5) Authorization/consent screen (best-effort)
        let auth_btn = driver
            .find_xpath(
                "//button[@type='submit' or contains(text(), 'Continue') or contains(text(), 'Authorize')]",
            )
            .await;
        if let Ok(b) = auth_btn {
            let _ = driver.click(&b).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        // 7) Wait for redirect with request_token
        // On some setups the redirect target immediately 302s to a frontend URL
        // (e.g. `.../capture_request_token?...&request_token=...` -> `.../dashboard?...`),
        // which can remove `request_token` very quickly. So we poll fast and
        // grab the first URL that contains the token.
        let final_url = wait_for_request_token_url(driver, redirect_url, Duration::from_secs(60)).await?;

        let request_token = extract_query_param(&final_url, "request_token").ok_or_else(|| {
            AppError::KiteApi(format!(
                "Could not find request_token in observed redirect URL. Last URL: {final_url}"
            ))
        })?;

        Ok(request_token)
    }
    .await;

    if r.is_err() && options.debug {
        let _ = write_debug_screenshot(driver, user_id).await;
        let _ = write_debug_page_source(driver, user_id).await;
        let cur = driver.current_url().await.unwrap_or_default();
        println!("AutoLogin[debug]: last_url={cur}");
    }

    r
}

async fn wait_for_request_token_url(
    driver: &WebDriver,
    _redirect_url: &str,
    timeout: Duration,
) -> Result<String, AppError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = String::new();
    loop {
        let cur = driver.current_url().await.unwrap_or_default();
        if !cur.is_empty() {
            last = cur.clone();
        }
        if cur.contains("request_token=") {
            return Ok(cur);
        }
        // If we at least reached the configured redirect_url, keep polling fast
        // for a bit longer; the token may appear on an intermediate URL.
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::KiteApi(format!(
                "Timeout waiting for request_token in URL. Last URL: {last}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn safe_send_keys(driver: &WebDriver, el: &selenium::Element, text: &str) -> Result<(), AppError> {
    let _ = driver.click(el).await;
    let _ = driver.clear(el).await;
    driver.send_keys(el, text).await
}

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    for (k, v) in parsed.query_pairs() {
        if k == key {
            return Some(v.to_string());
        }
    }
    None
}

fn generate_totp(secret_b32: &str) -> Result<String, AppError> {
    // Equivalent of pyotp.TOTP(secret).now(), 30s window, 6 digits.
    let cleaned = secret_b32.replace(' ', "").to_uppercase();
    let key = base32::decode(base32::Alphabet::RFC4648 { padding: false }, cleaned.as_str())
        .ok_or_else(|| AppError::KiteApi("Invalid base32 totp_secret".to_string()))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| AppError::KiteApi(e.to_string()))?
        .as_secs();
    let counter = now / 30;
    let msg = counter.to_be_bytes();

    let mut mac = Hmac::<Sha1>::new_from_slice(&key)
        .map_err(|e| AppError::KiteApi(format!("HMAC init failed: {e}")))?;
    mac.update(&msg);
    let hash = mac.finalize().into_bytes();
    let offset = (hash[19] & 0x0f) as usize;
    let bin_code = ((u32::from(hash[offset]) & 0x7f) << 24)
        | (u32::from(hash[offset + 1]) << 16)
        | (u32::from(hash[offset + 2]) << 8)
        | u32::from(hash[offset + 3]);
    let otp = bin_code % 1_000_000;
    Ok(format!("{:06}", otp))
}

fn spawn_chromedriver(path: &str, port: u16) -> Result<Child, AppError> {
    let child = std::process::Command::new(path)
        .arg(format!("--port={port}"))
        .spawn()
        .map_err(|e| AppError::KiteApi(format!("Failed to spawn chromedriver '{path}': {e}")))?;
    Ok(child)
}

async fn write_debug_screenshot(driver: &WebDriver, user_id: &str) -> Result<(), AppError> {
    let b64 = driver.screenshot_base64().await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| AppError::KiteApi(format!("Failed to decode screenshot base64: {e}")))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| AppError::KiteApi(e.to_string()))?
        .as_secs();
    let file = format!("autologin_failure_{user_id}_{ts}.png");
    std::fs::write(&file, bytes)
        .map_err(|e| AppError::KiteApi(format!("Failed to write screenshot '{file}': {e}")))?;
    println!("AutoLogin[debug]: wrote screenshot: {file}");
    Ok(())
}

async fn write_debug_page_source(driver: &WebDriver, user_id: &str) -> Result<(), AppError> {
    let html = driver.page_source().await?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| AppError::KiteApi(e.to_string()))?
        .as_secs();
    let file = format!("autologin_failure_{user_id}_{ts}.html");
    std::fs::write(&file, html)
        .map_err(|e| AppError::KiteApi(format!("Failed to write page source '{file}': {e}")))?;
    println!("AutoLogin[debug]: wrote page source: {file}");
    Ok(())
}

fn default_chrome_binary(os_type: &str) -> Option<&'static str> {
    match os_type {
        "macos" | "darwin" => Some("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        "ubuntu" | "linux" => Some("/usr/bin/google-chrome"),
        _ => None,
    }
}

fn find_default_chromedriver(os_type: &str) -> Option<String> {
    let candidates: &[&str] = match os_type {
        "macos" | "darwin" => &[
            "/usr/local/bin/chromedriver",
            "/opt/homebrew/bin/chromedriver",
        ],
        "ubuntu" | "linux" => &["/usr/local/bin/chromedriver", "/usr/bin/chromedriver"],
        _ => &[],
    };

    for p in candidates {
        if Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    None
}

fn choose_chromedriver_spawn_path(
    chromedriver_path_override: Option<String>,
    chromedriver_path_from_db: Option<String>,
    os_type: &str,
) -> Option<String> {
    // 1) If CHROMEDRIVER_PATH is set and exists, use it.
    // 2) If CHROMEDRIVER_PATH is set but stale (common after renames), ignore it and try
    //    auto-detecting a pinned driver under the repo's `.drivers/`.
    // 3) Otherwise prefer DB path, then `.drivers/`, then system paths.
    if let Some(p) = chromedriver_path_override.clone() {
        let p_trim = p.trim();
        if !p_trim.is_empty() {
            if Path::new(p_trim).exists() {
                return Some(p_trim.to_string());
            }
            println!("AutoLogin: ignoring CHROMEDRIVER_PATH (not found): {p_trim}");
            if let Some(pinned) = find_repo_drivers_chromedriver(os_type) {
                return Some(pinned);
            }
        }
    }

    if let Some(p) = chromedriver_path_from_db.as_deref() {
        let p = p.trim();
        if !p.is_empty() && Path::new(p).exists() {
            return Some(p.to_string());
        }
    }

    find_repo_drivers_chromedriver(os_type).or_else(|| find_default_chromedriver(os_type))
}

fn find_repo_drivers_chromedriver(os_type: &str) -> Option<String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let drivers_dir = manifest_dir.join(".drivers");
    if !drivers_dir.exists() {
        return None;
    }

    // Scan `.drivers/` for any file literally named `chromedriver`.
    // This is intentionally version-agnostic so future driver versions work without code changes.
    find_best_file_named(&drivers_dir, "chromedriver", os_type, 6)
}


fn find_best_file_named(dir: &Path, filename: &str, os_type: &str, depth_left: usize) -> Option<String> {
    let mut found: Vec<String> = Vec::new();
    collect_files_named(dir, filename, depth_left, &mut found);
    if found.is_empty() {
        return None;
    }

    // Deterministic choice: score by OS/arch hints, then sort.
    let arch = std::env::consts::ARCH;
    found.sort_by(|a, b| {
        let sa = chromedriver_path_score(a, os_type, arch);
        let sb = chromedriver_path_score(b, os_type, arch);
        sb.cmp(&sa).then_with(|| a.cmp(b))
    });
    Some(found[0].clone())
}

fn collect_files_named(dir: &Path, filename: &str, depth_left: usize, out: &mut Vec<String>) {
    if depth_left == 0 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for e in entries.flatten() {
        let path: PathBuf = e.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name == filename {
                    out.push(path.to_string_lossy().to_string());
                }
            }
        } else if path.is_dir() {
            collect_files_named(&path, filename, depth_left - 1, out);
        }
    }
}

fn chromedriver_path_score(path: &str, os_type: &str, arch: &str) -> i32 {
    let p = path.to_ascii_lowercase();
    let mut score = 0;

    // Prefer known OS directory hints.
    match os_type {
        "macos" | "darwin" => {
            if p.contains("mac") {
                score += 50;
            }
            if arch == "aarch64" {
                if p.contains("arm64") || p.contains("aarch64") {
                    score += 40;
                }
            }
            if arch == "x86_64" {
                if p.contains("x64") || p.contains("x86_64") {
                    score += 40;
                }
            }
        }
        "ubuntu" | "linux" => {
            if p.contains("linux") {
                score += 50;
            }
            if arch == "x86_64" {
                if p.contains("64") || p.contains("x86_64") {
                    score += 20;
                }
            }
        }
        _ => {}
    }

    // Prefer paths that look like Chrome-for-Testing bundles.
    if p.contains("chromedriver") {
        score += 5;
    }
    if p.contains("chrome") {
        score += 1;
    }

    score
}
