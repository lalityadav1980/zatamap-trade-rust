use axum::{routing::get, Router};

pub fn router() -> Router<crate::core::AppState> {
    Router::new()
        .route("/api/health", get(health::health))
        .route("/api/kite/login_url", get(kite::login_url))
        .route("/api/kite/callback", get(kite::callback))
}

mod health {
    use axum::{extract::State, Json};
    use serde_json::json;

    use crate::core::AppState;

    pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
        let db_ok = state.db.health().await.unwrap_or(false);
        Json(json!({"status": "ok", "db": db_ok}))
    }
}

mod kite {
    use axum::{
        extract::{Query, State},
        http::StatusCode,
        Json,
    };
    use serde::Deserialize;
    use serde_json::json;

    use crate::{core::AppState, dao::profile_dao, kite::auth};

    #[derive(Debug, Deserialize)]
    pub struct CallbackQuery {
        pub user_id: Option<String>,
        #[serde(rename = "userid")]
        pub userid: Option<String>,
        pub request_token: Option<String>,
        pub status: Option<String>,
        pub error: Option<String>,
    }

    pub async fn login_url(
        State(state): State<AppState>,
        Query(q): Query<CallbackQuery>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
        let user_id = q.user_id.or(q.userid)
            .ok_or((StatusCode::BAD_REQUEST, "Missing user_id/userid".to_string()))?;

        let creds = profile_dao::get_user_kite_creds_for_os(&state.db, &user_id, &state.config.os_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let creds = creds.ok_or((StatusCode::NOT_FOUND, "User not found".to_string()))?;

        let callback_url = auth::callback_url_for_user(&state.config.kite_callback_url, &user_id);
        let url = auth::login_url(&creds.api_key, &callback_url);
        Ok(Json(json!({"login_url": url})))
    }

    pub async fn callback(
        State(state): State<AppState>,
        Query(q): Query<CallbackQuery>,
    ) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
        if let Some(err) = q.error {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Kite callback error: {err}"),
            ));
        }
        if let Some(status) = &q.status {
            if status != "success" {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("Kite callback status not success: {status}"),
                ));
            }
        }
        let user_id = q.user_id.or(q.userid)
            .ok_or((StatusCode::BAD_REQUEST, "Missing user_id/userid".to_string()))?;

        let request_token = q
            .request_token
            .ok_or((StatusCode::BAD_REQUEST, "Missing request_token".to_string()))?;

        let creds = profile_dao::get_user_kite_creds_for_os(&state.db, &user_id, &state.config.os_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let creds = creds.ok_or((StatusCode::NOT_FOUND, "User not found".to_string()))?;

        let session =
            auth::exchange_request_token(&creds.api_key, &creds.api_secret, &request_token)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

        let updated = profile_dao::update_session_tokens_for_os(
            &state.db,
            &user_id,
            &state.config.os_type,
            &request_token,
            &session.access_token,
            session.public_token.as_deref(),
        )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if updated == 0 {
            return Err((StatusCode::NOT_FOUND, "User not found".to_string()));
        }

        Ok(Json(json!({
            "status": "stored",
            "user_id_param": user_id,
            "user_id": session.user_id,
            "public_token": session.public_token
        })))
    }
}
