use crate::db::Db;
use crate::core::AppError;

#[derive(Debug, Clone)]
pub struct UserKiteCreds {
    pub api_key: String,
    pub api_secret: String,
    pub access_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UserZerodhaLogin {
    pub api_key: String,
    pub api_secret: String,
    pub access_token: Option<String>,
    pub zerodha_password: Option<String>,
    pub zerodha_pin: Option<String>,
    pub totp_secret: Option<String>,
    pub os_type: Option<String>,
    pub chrome_binary_path: Option<String>,
    pub chromedriver_path: Option<String>,
}

pub async fn get_user_kite_creds(
    db: &Db,
    user_id: &str,
) -> Result<Option<UserKiteCreds>, AppError> {
    // Backward-compatible: if multiple rows exist (one per os_type), callers
    // should use `get_user_kite_creds_for_os`.
    let row = db
        .client()
        .query_opt(
            "SELECT api_key, api_secret, access_token FROM trade.profile WHERE userid = $1 ORDER BY updated_at DESC NULLS LAST LIMIT 1",
            &[&user_id],
        )
        .await?;

    Ok(row.map(|r| UserKiteCreds {
        api_key: r.get::<_, String>(0),
        api_secret: r.get::<_, String>(1),
        access_token: r.get::<_, Option<String>>(2),
    }))
}

pub async fn get_user_kite_creds_for_os(
    db: &Db,
    user_id: &str,
    os_type: &str,
) -> Result<Option<UserKiteCreds>, AppError> {
    let row = db
        .client()
        .query_opt(
            "SELECT api_key, api_secret, access_token FROM trade.profile WHERE userid = $1 AND os_type = $2",
            &[&user_id, &os_type],
        )
        .await?;

    Ok(row.map(|r| UserKiteCreds {
        api_key: r.get::<_, String>(0),
        api_secret: r.get::<_, String>(1),
        access_token: r.get::<_, Option<String>>(2),
    }))
}

pub async fn update_access_token(
    db: &Db,
    user_id: &str,
    access_token: &str,
) -> Result<u64, AppError> {
    let n = db
        .client()
        .execute(
            "UPDATE trade.profile SET access_token = $1, updated_at = NOW() WHERE userid = $2",
            &[&access_token, &user_id],
        )
        .await?;
    Ok(n)
}

pub async fn update_access_token_for_os(
    db: &Db,
    user_id: &str,
    os_type: &str,
    access_token: &str,
) -> Result<u64, AppError> {
    let n = db
        .client()
        .execute(
            "UPDATE trade.profile SET access_token = $1, updated_at = NOW() WHERE userid = $2 AND os_type = $3",
            &[&access_token, &user_id, &os_type],
        )
        .await?;
    Ok(n)
}

pub async fn update_session_tokens_for_os(
    db: &Db,
    user_id: &str,
    os_type: &str,
    request_token: &str,
    access_token: &str,
    public_token: Option<&str>,
) -> Result<u64, AppError> {
    let public_token: Option<String> = public_token.map(|s| s.to_string());
    let n = db
        .client()
        .execute(
            "UPDATE trade.profile SET request_token = $1, access_token = $2, public_token = $3, updated_at = NOW() WHERE userid = $4 AND os_type = $5",
            &[&request_token, &access_token, &public_token, &user_id, &os_type],
        )
        .await?;
    Ok(n)
}

pub async fn get_user_zerodha_login(
    db: &Db,
    user_id: &str,
) -> Result<Option<UserZerodhaLogin>, AppError> {
    let row = db
        .client()
        .query_opt(
            "SELECT api_key, api_secret, access_token, zerodha_password, zerodha_pin, totp_secret, os_type, chrome_binary_path, chromedriver_path FROM trade.profile WHERE userid = $1 ORDER BY updated_at DESC NULLS LAST LIMIT 1",
            &[&user_id],
        )
        .await?;

    Ok(row.map(|r| UserZerodhaLogin {
        api_key: r.get::<_, String>(0),
        api_secret: r.get::<_, String>(1),
        access_token: r.get::<_, Option<String>>(2),
        zerodha_password: r.get::<_, Option<String>>(3),
        zerodha_pin: r.get::<_, Option<String>>(4),
        totp_secret: r.get::<_, Option<String>>(5),
        os_type: r.get::<_, Option<String>>(6),
        chrome_binary_path: r.get::<_, Option<String>>(7),
        chromedriver_path: r.get::<_, Option<String>>(8),
    }))
}

pub async fn get_user_zerodha_login_for_os(
    db: &Db,
    user_id: &str,
    os_type: &str,
) -> Result<Option<UserZerodhaLogin>, AppError> {
    let row = db
        .client()
        .query_opt(
            "SELECT api_key, api_secret, access_token, zerodha_password, zerodha_pin, totp_secret, os_type, chrome_binary_path, chromedriver_path FROM trade.profile WHERE userid = $1 AND os_type = $2",
            &[&user_id, &os_type],
        )
        .await?;

    Ok(row.map(|r| UserZerodhaLogin {
        api_key: r.get::<_, String>(0),
        api_secret: r.get::<_, String>(1),
        access_token: r.get::<_, Option<String>>(2),
        zerodha_password: r.get::<_, Option<String>>(3),
        zerodha_pin: r.get::<_, Option<String>>(4),
        totp_secret: r.get::<_, Option<String>>(5),
        os_type: r.get::<_, Option<String>>(6),
        chrome_binary_path: r.get::<_, Option<String>>(7),
        chromedriver_path: r.get::<_, Option<String>>(8),
    }))
}
