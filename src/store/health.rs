use async_trait::async_trait;

use crate::error::AppError;

use super::SqliteStore;

/// readiness 프로브가 쓰는 "저장소에 실제로 닿는가" 확인.
// [설명] 메서드 하나짜리 작은 trait이지만 굳이 분리한다. readiness가 실제 업무 쿼리에
// 의존하면 "readiness를 위해 목록을 읽는" 식의 엉뚱한 결합이 생기기 쉽기 때문이다.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait HealthStore: Send + Sync {
    async fn ping(&self) -> Result<(), AppError>;
}

#[async_trait]
impl HealthStore for SqliteStore {
    // [설명] 이 프로젝트에서 유일하게 매크로(query!)를 쓰지 않는 쿼리다. `select 1`은
    // 스키마의 어떤 객체도 참조하지 않아 컴파일 타임에 검증할 것이 없고, 매크로로 바꾸면
    // 컬럼 이름이 그대로 `1`이라 Rust 식별자가 못 되어 별칭을 억지로 붙여야 한다.
    // 얻는 것 없이 노이즈만 늘어서 그대로 뒀다.
    //
    // [SQLite] 이 프로브의 의미가 Postgres 때와 조금 다르다. 그쪽은 "네트워크 너머 DB
    // 프로세스가 살아 있는가"를 물었지만, 여기서는 DB가 같은 프로세스 안의 라이브러리라
    // 사실상 "파일을 열어 읽을 수 있는가"를 확인하는 것에 가깝다. 그래도 디스크가 차거나
    // 파일 권한이 틀어진 경우를 잡아주므로 readiness 프로브로서의 값은 그대로다.
    async fn ping(&self) -> Result<(), AppError> {
        sqlx::query("select 1").execute(&self.pool).await?;
        Ok(())
    }
}
