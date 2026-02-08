use super::config::AppConfig;
use crate::db::Db;
use crate::ticks::TickStore;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub db: Arc<Db>,
    pub ticks: Arc<TickStore>,
}
