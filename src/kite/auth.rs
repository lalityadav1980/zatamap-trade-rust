use crate::core::AppError;
use crate::kite::types::{KiteEnvelope, SessionToken};
use reqwest::Url;
use sha2::{Digest, Sha256};

const KITE_BASE_URL: &str = "https://api.kite.trade";

pub fn login_url(api_key: &str, callback_url: &str) -> String {
    // By default we do NOT pass redirect_url here.
    // Kite already knows the Redirect URL configured for the API key, and in
    // some cases providing `redirect_url=` causes a 400:
    //   "supplied URL does not belong to the registered URL domain".
    //
    // When you *do* need to override, set `KITE_INCLUDE_REDIRECT_URL=1`.
    let include_redirect = std::env::var("KITE_INCLUDE_REDIRECT_URL")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !include_redirect {
        return format!(
            "https://kite.zerodha.com/connect/login?api_key={}&v=3",
            urlencoding::encode(api_key)
        );
    }

    // Minimal escaping for a URL embedded as a query param value.
    let redirect_url = encode_redirect_url_param(callback_url);
    format!(
        "https://kite.zerodha.com/connect/login?api_key={}&v=3&redirect_url={}",
        urlencoding::encode(api_key),
        redirect_url
    )
}

fn encode_redirect_url_param(input: &str) -> String {
    // Fully encode so browsers and server-side parsers don't misinterpret
    // inner `?`/`&` in the embedded URL.
    urlencoding::encode(input).to_string()
}

/// Build a per-user redirect URL.
///
/// Kite requires the `redirect_url` used in the login request to match what is
/// configured in the Kite developer console.
///
/// Supported forms:
/// - `https://.../capture_request_token?userid=QFH620` (already includes user)
/// - `https://.../capture_request_token?userid={userid}` (templated)
/// - `https://.../capture_request_token` (we append `userid=<user_id>`)
pub fn callback_url_for_user(base: &str, user_id: &str) -> String {
    if base.contains("{userid}") {
        return base.replace("{userid}", &urlencoding::encode(user_id));
    }
    if base.contains("{user_id}") {
        return base.replace("{user_id}", &urlencoding::encode(user_id));
    }

    if let Ok(mut url) = Url::parse(base) {
        let has_user = url
            .query_pairs()
            .any(|(k, _)| k == "userid" || k == "user_id");
        if has_user {
            return base.to_string();
        }

        url.query_pairs_mut().append_pair("userid", user_id);
        return url.to_string();
    }

    // Fallback for non-absolute URLs.
    if base.contains('?') {
        format!("{base}&userid={}", urlencoding::encode(user_id))
    } else {
        format!("{base}?userid={}", urlencoding::encode(user_id))
    }
}

pub async fn exchange_request_token(
    api_key: &str,
    api_secret: &str,
    request_token: &str,
) -> Result<SessionToken, AppError> {
    let checksum = checksum(api_key, request_token, api_secret);
    let url = format!("{KITE_BASE_URL}/session/token");

    let resp = reqwest::Client::new()
        .post(url)
        .form(&[
            ("api_key", api_key),
            ("request_token", request_token),
            ("checksum", checksum.as_str()),
        ])
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(AppError::KiteApi(format!("HTTP {status}: {text}")));
    }

    let envelope: KiteEnvelope<SessionToken> = serde_json::from_str(&text)?;
    match envelope.status.as_str() {
        "success" => envelope
            .data
            .ok_or_else(|| AppError::KiteApi("Missing data in response".to_string())),
        _ => Err(AppError::KiteApi(
            envelope
                .message
                .unwrap_or_else(|| "Unknown Kite error".to_string()),
        )),
    }
}

fn checksum(api_key: &str, request_token: &str, api_secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    hasher.update(request_token.as_bytes());
    hasher.update(api_secret.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}
