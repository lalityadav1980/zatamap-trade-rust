use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Missing required env var: {0}")]
    MissingEnv(&'static str),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Csv(#[from] csv::Error),

    #[error(transparent)]
    Db(#[from] tokio_postgres::Error),

    #[error("Kite API error: {0}")]
    KiteApi(String),
}
