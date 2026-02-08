use crate::core::AppState;
use axum::Router;

mod routes;

pub fn router(state: AppState) -> Router {
    Router::new().merge(routes::router()).with_state(state)
}
