use async_trait::async_trait;
// [아키텍처] 스케줄러가 "이 발화를 내가 맡는다"를 선언하는 포트. 예제가 아니라 골격이다
// (migrations/0002_scheduled_job_runs.sql과 짝).
//
// 이 포트가 있어서 scheduler/는 "중복 실행을 막는다"만 알고 그 수단(SQLite 기본키)은
// 모른다. 나중에 Redis SETNX나 etcd로 바꾸더라도 바뀌는 건 이 파일의 impl뿐이고,
// 무엇보다 스케줄러 루프의 단위 테스트가 DB 없이 mock으로 돈다.
use chrono::{DateTime, Utc};

use crate::error::AppError;

use super::SqliteStore;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait JobRunStore: Send + Sync {
    /// 이 job의 이 발화 시각을 선점한다. `true`면 내가 실행할 차례, `false`면 이미
    /// 실행된 발화라는 뜻이다(재기동 직후 같은 발화를 다시 집었을 때).
    async fn claim_run(
        &self,
        job_name: &str,
        scheduled_for: DateTime<Utc>,
    ) -> Result<bool, AppError>;

    /// 선점했던 실행을 마감한다. `error`가 `Some`이면 실패로 기록된다.
    // [Rust 특징] `'a`를 직접 적은 이유는 mockall 때문이다. `Option<&str>`처럼 참조가
    // 제네릭 타입 안에 들어가면 수명 생략이 통하지 않아, mock을 생성할 때 "명시적 수명이
    // 필요하다"는 에러가 난다. 위 `job_name: &str`처럼 인자 자리에 바로 온 참조는 괜찮다.
    async fn finish_run<'a>(
        &self,
        job_name: &'a str,
        scheduled_for: DateTime<Utc>,
        error: Option<&'a str>,
    ) -> Result<(), AppError>;

    /// `cutoff`보다 오래된 실행 기록을 지우고, 지운 행 수를 돌려준다.
    /// scheduler::prune_runs가 쓴다 — 이 테이블은 발화마다 한 줄씩 쌓이기 때문이다.
    async fn delete_runs_before(&self, cutoff: DateTime<Utc>) -> Result<u64, AppError>;
}

#[async_trait]
impl JobRunStore for SqliteStore {
    // [설명] `on conflict do nothing` + `rows_affected()`가 이 구조의 핵심이다. 이미
    // 그 발화의 행이 있으면 0행이 영향받고, 에러는 나지 않는다 — "졌다"가 예외가 아니라
    // 평범한 결과값이 되므로 호출부가 unique violation을 잡아 해석할 필요가 없다.
    //
    // [주의] "먼저 조회해서 없으면 넣는다"로 쓰면 안 된다. 조회와 삽입 사이에 다른 쪽이
    // 끼어들 수 있고, 그 창은 정확히 이 락이 막으려던 것이다. 판정은 반드시 DB의 원자적
    // 연산 하나로 끝나야 한다.
    //
    // [SQLite] `on conflict do nothing`은 SQLite도 그대로 지원한다(3.24+). 바뀐 것은
    // 플레이스홀더가 `?`라는 것과, started_at 기본값이 없어져 여기서 직접 넣는다는 것뿐이다.
    async fn claim_run(
        &self,
        job_name: &str,
        scheduled_for: DateTime<Utc>,
    ) -> Result<bool, AppError> {
        let started_at = Utc::now();

        let result = sqlx::query!(
            "insert into scheduled_job_runs (job_name, scheduled_for, started_at) \
             values (?, ?, ?) on conflict (job_name, scheduled_for) do nothing",
            job_name,
            scheduled_for,
            started_at
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() == 1)
    }

    // [설명] 마감 실패는 여기서 에러로 올리되, 호출부(scheduler)는 그것 때문에 루프를
    // 멈추지 않는다 — job은 이미 실행됐고 기록만 못 남긴 상황이라, 다음 발화를 포기할
    // 이유가 없기 때문이다.
    async fn finish_run<'a>(
        &self,
        job_name: &'a str,
        scheduled_for: DateTime<Utc>,
        error: Option<&'a str>,
    ) -> Result<(), AppError> {
        // [SQLite] Postgres의 now()에 해당하는 함수가 없다(CURRENT_TIMESTAMP는 초 단위의
        // 다른 형식이라 이 테이블의 다른 시각들과 형식이 어긋난다). 그래서 여기서도 값을
        // 만들어 바인딩한다 — 컬럼에 들어가는 문자열 형식이 언제나 한 가지로 유지된다.
        let finished_at = Utc::now();

        sqlx::query!(
            "update scheduled_job_runs set finished_at = ?, error = ? \
             where job_name = ? and scheduled_for = ?",
            finished_at,
            error,
            job_name,
            scheduled_for
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // [주의] 보관 기간이 너무 짧으면 안 된다. 이 테이블은 기록이자 **락**이라, 아직
    // 실행 중이거나 방금 끝난 발화의 행을 지우면 재기동 시 그 발화를 다시 선점할 수
    // 있다. 기준을 발화 주기보다 훨씬 길게(일 단위) 잡는다.
    async fn delete_runs_before(&self, cutoff: DateTime<Utc>) -> Result<u64, AppError> {
        // [SQLite] scheduled_for가 ISO 8601 문자열이라 `<` 비교가 문자열 비교로
        // 이뤄지지만, 이 형식은 문자열 정렬과 시간 정렬이 일치하므로 의도대로 동작한다.
        let result = sqlx::query!(
            "delete from scheduled_job_runs where scheduled_for < ?",
            cutoff
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}
