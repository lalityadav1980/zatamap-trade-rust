use crate::auth;
use crate::{core::AppError, core::AppState, dao::profile_dao};

/// Similar to a Python `initialize_on_startup` hook.
///
/// This keeps the structure ready for an auto-login flow.
/// For now, it validates DB connectivity and reports whether a per-user token exists.
pub async fn initialize_on_startup(state: &AppState) -> Result<(), AppError> {
    let ok = state.db.health().await?;
    println!("Startup: db_health={ok}");

    if let Some(user_id) = state.config.startup_autologin_user_id.as_deref() {
        let os_type = state
            .config
            .startup_autologin_os_type
            .as_deref()
            .unwrap_or(&state.config.os_type);
        println!("Startup: autologin user_id={user_id} os_type={os_type}");

        let creds = profile_dao::get_user_kite_creds_for_os(&state.db, user_id, os_type).await?;
        match creds {
            None => println!("Startup: AUTOLOGIN_USER_ID not found in trade.profile"),
            Some(c) => {
                let has_token = c
                    .access_token
                    .as_ref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
                println!("Startup: user={user_id} has_access_token={has_token}");

                let opts = auth::autologin::AutoLoginOptions {
                    debug: state.config.startup_autologin_debug,
                    force: state.config.startup_autologin_force,
                };

                // Mirror Python behavior: normally only log in if no token, unless forced.
                if !has_token || opts.force {
                    auth::autologin::maybe_autologin_for_os(state, user_id, os_type, opts).await?;
                }
            }
        }
    }

    Ok(())
}
