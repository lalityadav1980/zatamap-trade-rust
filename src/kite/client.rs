use crate::core::AppError;
use crate::kite::types::{Holding, KiteEnvelope, UserProfile};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;

const KITE_BASE_URL: &str = "https://api.kite.trade";

#[derive(Clone)]
pub struct KiteClient {
    http: reqwest::Client,
}

impl KiteClient {
    pub fn new(api_key: &str, access_token: &str) -> Result<Self, AppError> {
        let mut headers = HeaderMap::new();
        let auth = format!("token {api_key}:{access_token}");
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&auth).map_err(|e| AppError::KiteApi(e.to_string()))?,
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;
        Ok(Self { http })
    }

    pub async fn profile(&self) -> Result<UserProfile, AppError> {
        self.get("/user/profile").await
    }

    pub async fn holdings(&self) -> Result<Vec<Holding>, AppError> {
        self.get("/portfolio/holdings").await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, AppError> {
        let url = format!("{KITE_BASE_URL}{path}");
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            return Err(AppError::KiteApi(format!("HTTP {status}: {text}")));
        }

        let envelope: KiteEnvelope<T> = serde_json::from_str(&text)?;
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
}
