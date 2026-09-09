use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{error::AppError, session_service, state::AppState};

#[derive(Serialize, Clone)]
pub struct CurrentUser {
    pub id: String,
    pub login_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>
}

pub const SESSION_COOKIE_NAME: &str = "__Host-session";

impl FromRequestParts<Arc<AppState>> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {

        let cookies = CookieJar::from_headers(&parts.headers);
        let session_id = cookies.get(SESSION_COOKIE_NAME).ok_or(AppError::Unauthorized)?.value();
        let session = session_service::get_session(state.session_store.as_ref(), session_id).await?.ok_or(AppError::Unauthorized)?;
        if session.is_expired() {
            return Err(AppError::Unauthorized);
        }
        let user = state.user_store.get_user_by_id(&session.user_id).await?.ok_or(AppError::Unauthorized)?;

        Ok(CurrentUser {
            id : user.id,
            login_id : user.login_id,
            name: user.name,
            created_at: user.created_at,
            last_login_at: user.last_login_at
        })
    }
}