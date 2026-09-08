// [예제] HTTP 핸들러의 모양. 하는 일이 딱 세 단계뿐인 게 요점이다:
//   요청 파싱(Json/Path 추출) → domain 함수 호출 → 결과를 Json으로 감싸기.
// 검증도 SQL도 여기 없다. 핸들러가 얇으면 도메인 로직이 HTTP 없이 테스트되고, 같은 로직을
// CLI나 컨슈머에서 재사용할 수 있다.
use std::sync::Arc;

use axum::{Json, Router, extract::State, response::IntoResponse, routing::post};
use axum_extra::extract::{CookieJar, cookie::{Cookie, SameSite}};
use serde::Deserialize;

use crate::{
    error::AppError, session_service, state::AppState, user_service,
};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/login", post(login))
}


#[derive(Deserialize)]
struct LoginBody {
    login_id: String,
    password: String,
}

async fn login(
    jar: CookieJar, 
    State(state): State<Arc<AppState>>, 
    Json(body): Json<LoginBody>
) -> Result<(CookieJar, Json<()>), AppError> {


    let user_id = user_service::do_login(state.user_store.as_ref(), &body.login_id, &body.password).await?;
    let (session_id, _) = session_service::save_session(state.session_store.as_ref(), &user_id, state.config.session_age_days).await?;
    

    let session_cookie = Cookie::build(("__Host-session", session_id))
        .http_only(true)
        .secure(true)
        .max_age(time::Duration::days(state.config.session_age_days))
        .same_site(SameSite::Lax)
        .path("/")
        .build();

    Ok((jar.add(session_cookie), Json(())))

}

