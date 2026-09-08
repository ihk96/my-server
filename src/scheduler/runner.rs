// [아키텍처] 실행 기반. 크론식 파싱, job별 루프, 분산 락이 여기 있고 **등록은 여기서 하지
// 않는다** — 등록 창구는 registry.rs이고, 무엇을 언제 돌릴지는 domain/jobs.rs다.
use std::{collections::HashSet, str::FromStr, sync::Arc, time::Duration};

use anyhow::Context;
use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use croner::Cron;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{config::AppConfig, state::AppState};

use super::{
    prune,
    registry::{JobFuture, JobRegistry},
};

// [설명] 다음 발화까지 몇 시간이 남았더라도 한 번에 이만큼씩만 잔다. `tokio::time::sleep`은
// 단조 시계(monotonic clock)를 쓰므로, 노트북이 절전에서 깨거나 NTP가 시계를 크게 당기면
// "12시간 뒤"가 실제 벽시계로는 엉뚱한 시각이 된다. 짧게 끊어 자고 매번 벽시계로 남은
// 시간을 다시 계산하면 그 어긋남이 최대 이 값만큼으로 제한된다.
const MAX_SLEEP: Duration = Duration::from_secs(60);

/// 등록된 job들을 각자의 크론식대로 돌리는 실행기.
pub struct Scheduler {
    enabled: bool,
    jobs: Vec<ScheduledJob>,
}

struct ScheduledJob {
    name: &'static str,
    cron: Cron,
    tz: Tz,
    run: Box<dyn Fn(Arc<AppState>) -> JobFuture + Send + Sync>,
}

impl Scheduler {
    /// 등록부를 받아 스케줄러를 조립한다. 호출부는 `Scheduler::build(&cfg, jobs::register)`
    /// 한 줄이면 되고, job이 늘어도 이 시그니처는 그대로다.
    // [아키텍처] 등록부를 값(`Vec<Job>`)이 아니라 **함수**로 받는 이유는, 스케줄러가 자기
    // job(prune)을 먼저 등록한 뒤 창구를 넘겨줘야 하기 때문이다. 그래서 스케줄러의 유지보수
    // job이 domain/jobs.rs에 드러나지 않는다.
    pub fn build(cfg: &AppConfig, register: impl FnOnce(&mut JobRegistry)) -> anyhow::Result<Self> {
        // [주의] 보관 기간이 0 이하면 prune이 방금 끝난 발화의 행까지 지운다. 그 행은
        // 기록이자 **락**이라, 사라지면 같은 발화를 다른 인스턴스가 다시 선점할 수 있다.
        anyhow::ensure!(
            cfg.scheduler_run_retention_days >= 1,
            "SCHEDULER_RUN_RETENTION_DAYS must be at least 1 (got {})",
            cfg.scheduler_run_retention_days
        );

        let mut registry = JobRegistry::new();
        prune::register(
            &mut registry,
            TimeDelta::days(cfg.scheduler_run_retention_days),
        );
        register(&mut registry);

        Self::from_registry(registry, cfg.scheduler_enabled)
    }

    /// 등록부가 실제로 조립되는지만 확인한다(기동 없이). 등록부의 테스트가 쓴다.
    pub fn validate(register: impl FnOnce(&mut JobRegistry)) -> anyhow::Result<()> {
        let mut registry = JobRegistry::new();
        register(&mut registry);
        Self::from_registry(registry, true).map(|_| ())
    }

    // [아키텍처] 크론식 파싱을 미리 해두는 것이 요점이다. 크론식은 문자열이라 오타가 컴파일
    // 타임에 걸리지 않는데, 이 검증이 루프 안에 있으면 "배포는 성공했는데 그 job만 조용히
    // 안 도는" 상태가 된다 — 그리고 그건 다음 발화 시각이 지나야 드러난다. 기동 시점에
    // 에러로 올려 프로세스가 아예 뜨지 않게 하는 편이 낫다(config.rs와 같은 규칙).
    fn from_registry(registry: JobRegistry, enabled: bool) -> anyhow::Result<Self> {
        let mut names = HashSet::new();
        let mut jobs = Vec::new();

        for job in registry.into_jobs() {
            anyhow::ensure!(
                names.insert(job.name),
                "duplicate scheduled job name: {} (the name is the distributed lock key)",
                job.name
            );

            let cron = Cron::from_str(job.cron)
                .with_context(|| format!("invalid cron for job {}: {:?}", job.name, job.cron))?;

            jobs.push(ScheduledJob {
                name: job.name,
                cron,
                tz: job.tz,
                run: job.run,
            });
        }

        Ok(Self { enabled, jobs })
    }

    /// 종료 신호를 받을 때까지 모든 job을 돌린다.
    // [아키텍처] job마다 태스크를 하나씩 띄운다. 한 루프에서 전부 돌리면 오래 걸리는 job이
    // 그 시간 동안 다른 job의 발화를 통째로 밀어버린다 — 스케줄러에서 가장 흔한 사고다.
    //
    // [아키텍처] 종료에서 진짜 중요한 건 **취소를 어디서 관찰하느냐**다. 아래 루프는 job
    // 실행 도중이 아니라 **다음 발화를 기다리는 sleep**에서 취소를 보므로, 신호가 실행 중에
    // 도착해도 그 실행은 끝까지 마친다. 반대로 job 쪽에 select!를 걸면 지는 Future가 그
    // 자리에서 drop되어 작업이 중간에 잘린다 — DB에 절반만 반영된 채로 끝날 수 있다는 뜻이다.
    // graceful이라는 말이 가리키는 게 정확히 이 차이다. 그 대가로 종료가 한 사이클만큼
    // 늦어질 수 있어서, 호출부(server::run)가 기다리는 시간에 상한을 둔다.
    pub async fn run(self, state: Arc<AppState>, shutdown: CancellationToken) {
        if !self.enabled {
            tracing::info!("scheduler is disabled, no jobs will run");
            return;
        }

        let mut tasks = JoinSet::new();
        for job in self.jobs {
            tracing::info!(job = job.name, cron = %job.cron.pattern.as_str(), tz = %job.tz, "scheduled job registered");
            tasks.spawn(run_job(job, state.clone(), shutdown.clone()));
        }

        // [Rust 특징] JoinSet은 태스크가 끝나는 대로 하나씩 돌려준다. 여기서는 결과가
        // 필요 없고 "전부 끝날 때까지"만 기다리면 되므로 비어 있을 때까지 받아낸다.
        while tasks.join_next().await.is_some() {}

        tracing::info!("scheduler stopped");
    }
}

async fn run_job(job: ScheduledJob, state: Arc<AppState>, shutdown: CancellationToken) {
    loop {
        // [설명] 다음 발화 시각은 매번 **지금**을 기준으로 다시 계산한다. 그래서 프로세스가
        // 꺼져 있던 동안 지나간 발화는 따라잡지 않고 건너뛴다(catch-up 없음). 밀린 배치가
        // 기동 직후에 한꺼번에 쏟아지는 쪽이 대개 더 위험해서 택한 기본값이고, 놓친 발화가
        // 있었는지는 scheduled_job_runs 테이블에 빈 자리로 남으므로 사후에 알 수 있다.
        let now = Utc::now().with_timezone(&job.tz);
        let next = match job.cron.find_next_occurrence(&now, false) {
            Ok(next) => next.with_timezone(&Utc),
            // 파싱은 기동 때 끝났으므로 여기까지 오는 실패는 "이 패턴에 다음 발화가 없다"
            // (예: 존재하지 않는 2월 30일) 같은 경우다. 다시 계산해도 결과가 같으니 루프를
            // 계속 도는 것은 무의미하다.
            Err(err) => {
                tracing::error!(job = job.name, error = %err, "no upcoming occurrence, job disabled");
                return;
            }
        };
        tracing::debug!(job = job.name, next = %next, "next run scheduled");

        if !sleep_until(next, &shutdown).await {
            break;
        }

        fire(&job, &state, next).await;
    }

    tracing::info!(job = job.name, "scheduled job stopped");
}

/// 발화 하나를 선점하고 실행한다.
async fn fire(job: &ScheduledJob, state: &Arc<AppState>, scheduled_for: DateTime<Utc>) {
    // [아키텍처] 인스턴스가 여러 개여도 한 번만 돌게 하는 지점. 발화 시각은 크론식에서
    // 나오므로 모든 인스턴스가 같은 값을 계산하고, 그 값을 먼저 기록한 하나만 실행한다.
    match state.job_run_store.claim_run(job.name, scheduled_for).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(job = job.name, "run claimed by another instance, skipping");
            return;
        }
        // DB가 잠깐 죽은 경우다. 선점 여부를 모르는 채로 실행하면 중복 실행이 될 수 있으니
        // 이번 발화는 건너뛰고 루프는 유지한다.
        Err(err) => {
            tracing::warn!(job = job.name, error = %err, "could not claim run, skipping");
            return;
        }
    }

    tracing::info!(job = job.name, scheduled_for = %scheduled_for, "job started");

    // [Rust 특징] job 본체를 `tokio::spawn`으로 한 겹 감싼 이유는 **패닉 격리**다. 그냥
    // await하면 job 안의 패닉이 이 루프까지 풀려 올라가 그 job이 영영 멈춘다. 태스크 경계를
    // 하나 두면 패닉이 `JoinError`라는 값으로 잡히므로, 실패로 기록하고 다음 발화를 기다릴 수
    // 있다. (job의 future가 `Send + 'static`이라 이 감싸기가 공짜로 성립한다.)
    let error = match tokio::spawn((job.run)(state.clone())).await {
        Ok(Ok(())) => None,
        Ok(Err(err)) => Some(err.to_string()),
        Err(join_err) => Some(format!("job panicked: {join_err}")),
    };

    match &error {
        None => tracing::info!(job = job.name, "job finished"),
        Some(message) => tracing::error!(job = job.name, error = %message, "job failed"),
    }

    // 마감 기록에 실패해도 루프는 계속 간다 — job은 이미 실행됐고 기록만 못 남긴 상황이라,
    // 다음 발화를 포기할 이유가 없다.
    if let Err(err) = state
        .job_run_store
        .finish_run(job.name, scheduled_for, error.as_deref())
        .await
    {
        tracing::warn!(job = job.name, error = %err, "could not record job outcome");
    }
}

/// 벽시계로 `target`이 될 때까지 잔다. 종료 신호를 받아 중간에 깨면 `false`.
async fn sleep_until(target: DateTime<Utc>, shutdown: &CancellationToken) -> bool {
    loop {
        let remaining = (target - Utc::now()).to_std().unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            return true;
        }

        tokio::select! {
            _ = shutdown.cancelled() => return false,
            _ = tokio::time::sleep(remaining.min(MAX_SLEEP)) => {}
        }
    }
}
