use crate::core::AppState;
use axum::Router;
use tower_http::trace::TraceLayer;

mod routes;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(routes::router())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
