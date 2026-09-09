use std::sync::Arc;

use axum::{Json, Router, routing::get};

use crate::{error::AppError, routes::auth::CurrentUser, state::AppState};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/users/me",get(get_me))
}


async fn get_me(user: CurrentUser) -> Result<Json<CurrentUser>, AppError> {
    
    Ok(Json(user))
}