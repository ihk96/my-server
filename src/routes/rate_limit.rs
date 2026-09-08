// [아키텍처] 레이트 리밋 설정과 그 뒷정리. HTTP 레이어의 관심사이므로 routes/ 아래 둔다.
//
// [설계] 앱에서 거는 이유. 앞단의 Caddy에 맡길 수도 있지만, Caddy는 표준 빌드에 레이트
// 리밋이 없어서 플러그인을 넣어 직접 빌드해야 한다. 개인 서버에서 프록시를 커스텀 빌드로
// 관리하는 부담을 이것 하나 때문에 지는 것보다, 앱에 두고 엔드포인트별로 다르게 거는 쪽이
// 낫다고 판단했다(지금도 /health, /ready는 제외한다 — routes/mod.rs 참고).
use std::{sync::Arc, time::Duration};

use anyhow::Context;
use governor::middleware::NoOpMiddleware;
use tokio_util::sync::CancellationToken;
use tower_governor::{governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor};

use crate::config::AppConfig;

/// 버킷을 비우는 주기. 짧게 잡을 이유가 없다 — 메모리 회수가 목적이지 정확성에 영향을
/// 주지 않는다(오래된 버킷은 어차피 가득 찬 상태라, 지워지든 남아 있든 판정이 같다).
const PRUNE_INTERVAL: Duration = Duration::from_secs(300);

/// 이 서비스가 쓰는 레이트 리밋 설정의 구체 타입.
// [Rust 특징] GovernorConfig는 키 추출 방식과 미들웨어를 타입 파라미터로 받는다. 그대로
// 쓰면 시그니처마다 이 긴 타입이 반복되므로 별칭을 둔다.
pub type Config = tower_governor::governor::GovernorConfig<SmartIpKeyExtractor, NoOpMiddleware>;

/// 설정에서 레이트 리밋 구성을 만든다. 값이 유효하지 않으면 기동을 실패시킨다.
pub fn config(cfg: &AppConfig) -> anyhow::Result<Arc<Config>> {
    anyhow::ensure!(
        cfg.rate_limit_burst > 0,
        "RATE_LIMIT_BURST must be at least 1 (got {})",
        cfg.rate_limit_burst
    );
    anyhow::ensure!(
        cfg.rate_limit_per_second > 0,
        "RATE_LIMIT_PER_SECOND must be at least 1 (got {})",
        cfg.rate_limit_per_second
    );

    // [주의] tower_governor의 `per_second(n)`은 "초당 n개"가 **아니라** "n초마다 1개를
    // 보충한다"는 간격이다. 이름만 보고 쓰면 의도와 정확히 반대로 설정된다(초당 5개를
    // 원하면서 per_second(5)를 넣으면 5초당 1개가 된다).
    //
    // 설정값은 사람이 읽는 대로 "초당 몇 개"로 두고, 여기서 간격으로 뒤집는다.
    let period = Duration::from_secs(1) / cfg.rate_limit_per_second;

    let config = GovernorConfigBuilder::default()
        .period(period)
        .burst_size(cfg.rate_limit_burst)
        // [핵심] 클라이언트 IP를 헤더(x-forwarded-for 등)에서 먼저 찾고, 없으면 커넥션의
        // peer IP로 떨어진다. 앞에 Caddy를 두는 구성이라 이 선택이 맞다 — peer IP만 보면
        // 모든 요청이 프록시 IP 하나로 묶여서, 한 사람이 전체를 막아버릴 수 있다.
        //
        // [주의] 이 헤더는 **위조할 수 있다.** 이 설정이 안전한 것은 서버가
        // 127.0.0.1에만 바인딩되어(SERVER_ADDR) 외부에서 앱에 직접 닿을 수 없고, 따라서
        // 헤더를 붙이는 주체가 Caddy뿐이기 때문이다. 앱을 0.0.0.0으로 열어 공개하는 구성으로
        // 바꾸면 이 줄은 곧바로 우회 수단이 되므로, 그때는 PeerIpKeyExtractor로 되돌릴 것.
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .context("failed to build the rate limiter configuration")?;

    Ok(Arc::new(config))
}

/// 오래된 IP 버킷을 주기적으로 비운다. 종료 신호를 받으면 끝난다.
// [주의] 이 태스크가 없으면 IP별 버킷이 **계속 쌓이기만 한다.** governor의 keyed
// limiter는 스스로 줄어들지 않기 때문이다. 요청량이 적어도 시간이 지나면서 서로 다른
// IP가 누적되므로, 오래 켜 두는 서비스라면 반드시 있어야 한다.
//
// [설명] 크레이트의 공식 예제는 이것을 `std::thread::spawn`의 무한 루프로 띄운다. 그러면
// 종료 신호를 볼 방법이 없어서 프로세스가 내려갈 때까지 남는데, 이 서비스는 종료를
// 토큰 하나로 다루므로 같은 규칙에 맞춘다(server/mod.rs).
pub async fn prune(config: Arc<Config>, shutdown: CancellationToken) {
    let mut ticker = tokio::time::interval(PRUNE_INTERVAL);
    // 첫 tick은 즉시 발화하므로 한 번 흘려보낸다 — 기동하자마자 빈 맵을 청소할 이유가 없다.
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let limiter = config.limiter();
                let before = limiter.len();
                limiter.retain_recent();
                tracing::debug!(before, after = limiter.len(), "pruned rate limit buckets");
            }
            _ = shutdown.cancelled() => {
                tracing::info!("rate limit pruner stopped");
                return;
            }
        }
    }
}
