// [아키텍처] HTTP 레이어의 조립 지점. 각 하위 모듈이 자기 라우트 그룹을
// `Router<Arc<AppState>>`로 만들어 반환하면, 여기서 하나로 merge하고 공통 State/미들웨어를
// 딱 한 번 붙인다 — 라우트가 늘어나도 이 파일에 추가되는 건 `.merge()` 한 줄뿐이다.
//
// 새 라우트 그룹을 추가할 때 중앙 파일을 한 줄 건드려야 하는 건 의도된 트레이드오프다.
// Rust에는 런타임 리플렉션이 없어 애노테이션 스캐닝이 불가능하고, 링크 타임 등록
// (linkme/inventory)은 등록 순서 제어가 안 되고 컴파일러 검증도 못 받는다. 대신 이 파일
// 하나를 읽으면 서비스가 노출하는 라우트 전부를 알 수 있다.
mod example;
mod health;
mod login;
mod user;
pub mod rate_limit;
pub mod auth;

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    extract::Request,
    http::{header, HeaderValue, Method},
    Router,
};
use tower::ServiceBuilder;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor};
use tower_http::{
    catch_panic::CatchPanicLayer,
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::{config::AppConfig, state::AppState};

/// 요청 식별자를 실어 나르는 헤더.
// [설명] 요청에 이미 이 헤더가 있으면 그 값을 그대로 쓰고, 없을 때만 새로 만든다.
// 앞단(로드밸런서, 게이트웨이, 호출한 다른 서비스)이 붙여준 값을 이어받는 게 핵심이다 —
// 여기서 매번 새로 발급해버리면 서비스 경계를 넘는 순간 추적이 끊겨서, 요청 하나를
// 시스템 전체에서 따라갈 수 없게 된다.
const REQUEST_ID_HEADER: &str = "x-request-id";

/// 라우터를 조립한다. 설정이 유효하지 않으면(CORS 오리진 형식, 레이트 리밋 값) 여기서
/// 기동을 실패시킨다 — 잘못된 설정으로 뜬 서버는 겉보기에 정상이라 한참 뒤에 엉뚱한
/// 증상으로만 발견되기 때문이다.
pub fn build(
    state: Arc<AppState>,
    cfg: &AppConfig,
    rate_limit: Arc<rate_limit::Config>,
) -> anyhow::Result<Router> {
    // [설계] 레이트 리밋을 라우터 **전체**가 아니라 업무 라우트에만 건다.
    //
    // /health와 /ready는 모니터링이나 systemd가 주기적으로 때리는 곳이라, 함께 묶으면
    // 정상적인 감시 트래픽이 리밋을 갉아먹는다. 더 나쁜 경우는 리밋에 걸린 헬스체크가
    // 429를 받아 "서비스가 죽었다"고 판정되면서, 부하가 몰린 순간에 재시작까지 유발하는
    // 것이다 — 막으려던 상황을 오히려 악화시킨다.
    let limited = Router::new()
        .merge(user::routes())
        .layer(GovernorLayer::new(rate_limit));

    let login_limit = GovernorConfigBuilder::default()
        .period(Duration::from_secs(1))
        .burst_size(1)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .context("failed to build the rate limiter configuration")?;
    let login_limited = login::routes().layer(GovernorLayer::new(login_limit));


    // [설명] 오리진이 하나도 없으면 None이 되고, 아래 option_layer가 레이어를 통째로
    // 건너뛴다 — "허용 목록이 빈 CORS"를 붙여 모든 브라우저 요청을 막는 것과는 다르다.
    let cors = cors_layer(&cfg.cors_allowed_origins)?;

    let router = Router::new()
        .merge(health::routes())
        .merge(login_limited)
        .merge(limited)
        .with_state(state)
        // [아키텍처] 미들웨어는 ServiceBuilder로 쌓는다. Router::layer를 여러 번 부르면
        // 나중에 붙인 것이 바깥이 되어 코드에 적힌 순서와 실행 순서가 뒤집히는데,
        // ServiceBuilder는 **먼저 쓴 layer가 바깥**이라 읽는 순서가 곧 요청이 통과하는
        // 순서다. 아래처럼 순서 자체가 의미를 갖는 스택에서는 헷갈릴 여지를 없애는 게 낫다.
        .layer(
            ServiceBuilder::new()
                // 1. 가장 먼저 요청 식별자를 확정한다. 뒤따르는 모든 레이어와 핸들러가 이
                //    값을 볼 수 있어야 로그가 요청 단위로 묶인다.
                .layer(SetRequestIdLayer::new(
                    REQUEST_ID_HEADER.parse().unwrap(),
                    MakeRequestUuid,
                ))
                // 2. 그 다음이 로깅. span에 request_id를 실어 두면 이 요청을 처리하는 동안
                //    남는 모든 로그에 자동으로 따라붙어서, 동시 요청이 뒤섞여도 구분된다.
                //    이게 없으면 "3시 12분에 에러가 났다"는 문의를 로그에서 특정할 수 없다.
                //
                //    TraceLayer의 기본 레벨은 DEBUG인데, 그러면 `RUST_LOG=info`로는 안 보이고
                //    tower_http 전체에 debug를 켜야 해서 관련 없는 내부 로그까지 딸려온다.
                .layer(
                    TraceLayer::new_for_http()
                        .make_span_with(|req: &Request| {
                            let request_id = req
                                .headers()
                                .get(REQUEST_ID_HEADER)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or("-");

                            tracing::info_span!(
                                "request",
                                method = %req.method(),
                                uri = %req.uri(),
                                request_id = %request_id,
                            )
                        })
                        .on_response(DefaultOnResponse::new().level(Level::INFO)),
                )
                // 3. 패닉 잡기는 로깅 **안쪽**이어야 한다. 바깥에 두면 패닉으로 만들어진
                //    500이 위 span 밖에서 만들어져 로그에 남지 않고, 서버는 응답했는데
                //    기록은 없는 상태가 된다. 이 레이어가 아예 없으면 패닉 시 응답 없이
                //    연결만 끊겨서 클라이언트는 원인을 알 방법이 없다.
                .layer(CatchPanicLayer::new())
                // 4. CORS. 허용할 오리진은 CORS_ALLOWED_ORIGINS로 주입하며, 비어 있으면
                //    레이어 자체를 붙이지 않는다(= CORS 헤더가 나가지 않는다). 서버 간
                //    호출이나 같은 오리진에서 프록시로 붙는 구성이라면 그게 맞다 — CORS는
                //    브라우저가 강제하는 규칙이지 서버를 보호하는 장치가 아니기 때문이다.
                //
                //    [설계] 이걸 앞단의 Caddy가 아니라 앱에서 하는 이유는, "어떤 오리진에
                //    어떤 메서드를 허용하는가"가 API가 아는 지식이기 때문이다. 프록시에서
                //    하려면 preflight(OPTIONS) 응답과 Vary 헤더를 손으로 맞춰야 해서
                //    조용히 틀리기 쉽다.
                //
                //    [주의] `CorsLayer::permissive()`를 쓰지 않았다. 그건 모든 오리진을
                //    허용한다는 뜻이고, 한번 그렇게 열어두면 "동작하니까" 그대로 배포로
                //    넘어가기 쉽다. 쿠키나 Authorization 헤더를 실어 보내야 한다면
                //    `.allow_credentials(true)`가 추가로 필요한데, 그 경우 브라우저가
                //    와일드카드 오리진을 거부하므로 어차피 열거하는 수밖에 없다.
                .option_layer(cors)
                // 5. 타임아웃은 핸들러 바로 위. 초과하면 408로 끊는다. store::sqlite_pool의
                //    DB 타임아웃(acquire/busy)이 막지 못하는 구간 — 외부 HTTP 호출이
                //    응답하지 않는 경우 같은 — 을 여기서 막는다. 이것이 없으면 끝나지 않는
                //    요청이 쌓이면서 커넥션과 태스크를 계속 점유한다.
                //
                //    [주의] SQLite로 옮기면서 이 그물의 비중이 커졌다. Postgres에는 쿼리
                //    자체의 실행 시간을 끊는 statement_timeout이 있었지만 SQLite에는 그에
                //    해당하는 것이 없어서, "쿼리가 안 끝나는" 경우를 막는 것은 사실상
                //    이 타임아웃뿐이다.
                .layer(TimeoutLayer::new(Duration::from_secs(
                    cfg.request_timeout_secs,
                )))
                // 6. 마지막으로 확정된 식별자를 응답 헤더로 돌려준다. 클라이언트가 문의할 때
                //    이 값을 함께 주면 서버 로그에서 그 요청만 바로 집어낼 수 있다.
                .layer(PropagateRequestIdLayer::new(
                    REQUEST_ID_HEADER.parse().unwrap(),
                )),
        );

    Ok(router)
}

/// 허용 오리진 목록에서 CORS 레이어를 만든다. 목록이 비어 있으면 `None`(= CORS 끔).
fn cors_layer(origins: &[String]) -> anyhow::Result<Option<CorsLayer>> {
    if origins.is_empty() {
        return Ok(None);
    }

    // [설명] 오리진 형식 오류를 기동 시점에 드러낸다. 런타임에 조용히 무시되면
    // "CORS를 설정했는데 브라우저가 계속 막는다"가 되고, 그 원인이 오타라는 걸
    // 알아내기까지 한참 걸린다.
    let parsed = origins
        .iter()
        .map(|origin| {
            HeaderValue::from_str(origin)
                .with_context(|| format!("invalid CORS origin: {origin} (see .env.example)"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Some(
        CorsLayer::new()
            .allow_origin(parsed)
            // [주의] 여기 적힌 메서드/헤더만 브라우저의 preflight를 통과한다. 라우트에
            // PUT이나 DELETE를 추가하면 이 목록도 함께 늘려야 하며, 빠뜨리면 서버는
            // 정상인데 브라우저에서만 막히는 상태가 된다.
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE]),
    ))
}
