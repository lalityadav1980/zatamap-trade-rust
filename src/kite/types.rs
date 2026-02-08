use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub(crate) struct KiteEnvelope<T> {
    pub status: String,
    pub data: Option<T>,
    pub message: Option<String>,
    #[serde(default)]
    pub error_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UserProfile {
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub email: Option<String>,
    pub broker: Option<String>,
    pub exchanges: Option<Vec<String>>,
    pub products: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Holding {
    pub tradingsymbol: String,
    pub exchange: String,
    pub quantity: f64,
    pub average_price: f64,
    pub last_price: f64,
    pub pnl: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SessionToken {
    pub access_token: String,
    pub public_token: Option<String>,
    pub user_id: Option<String>,
}
