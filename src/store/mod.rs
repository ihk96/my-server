// [아키텍처] 저장소 계층. 각 하위 모듈이 "내가 제공하는 것"을 trait(포트)으로 공개하고,
// 그 trait의 SQLite 구현을 같은 파일에 함께 담는다 — domain/은 구현체가 아니라 trait에만
// 의존한다.
//
// trait의 모양을 정하는 기준은 **테이블이 아니라 쓰임새**다. 소비자가 한 번의 작업에
// 필요로 하는 단위로 메서드를 만들 것. 쿼리 단위로 잘게 쪼개면 재사용은 좋아 보이지만,
// 소비자 쪽 테스트가 매번 여러 스텁을 세워야 하고 조합 로직이 소비자에게 흩어진다.
// 포트와 그 구현을 한 파일에 두는 것도 같은 이유다 — 모양을 정할 때 둘을 같이 봐야 한다.
//
// 이 계층의 impl 블록에는 SQL을 쓰는 것 외에 판단하는 로직이 없으므로 의도적으로 단위
// 테스트 대상에서 제외한다 — SQL이 맞는지는 실제 DB에 붙여봐야만 알 수 있는 종류의 문제다.
pub mod health;
pub mod job_run;
pub mod user;
pub mod session;

use std::{str::FromStr, time::Duration};

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use crate::config::AppConfig;

pub use user::{UserRecord, UserStore};
pub use health::HealthStore;
pub use job_run::JobRunStore;

/// 설정에 따라 SQLite 커넥션 풀을 만든다.
// [아키텍처] 이 함수가 bin이 아니라 여기에 있는 이유는, 서버와 CLI 등 모든 진입점이 같은
// 풀 설정을 쓰게 만들기 위해서다. 이 코드가 bin 안에 있으면 다른 bin이 재사용할 수 없어서
// 결국 복사되고, 복사본은 시간이 지나면 원본과 갈라진다(타임아웃 하나가 한쪽에만 빠지는 식).
//
// 마이그레이션은 의도적으로 여기서 실행하지 않는다. 스키마를 바꾸는 건 서버 부팅 경로
// (server::run)의 책임이고, 운영자가 돌린 CLI가 조용히 스키마를 변경하면 놀라운 동작이 된다.
pub async fn sqlite_pool(cfg: &AppConfig) -> Result<SqlitePool, sqlx::Error> {
    // [설명] DATABASE_URL을 파싱한 뒤 이 서비스가 요구하는 PRAGMA들을 얹는다. URL에
    // 쿼리스트링으로 넣을 수도 있지만(`?mode=rwc` 등), 설정이 문자열 안에 숨는 것보다
    // 코드에 드러나는 편이 낫다 — 어떤 값으로 도는지 여기만 보면 된다.
    let options = SqliteConnectOptions::from_str(&cfg.database_url)?
        // 파일이 없으면 만든다. 서버가 마이그레이션까지 스스로 돌리므로, 새 환경에서는
        // 바이너리만 올리고 실행하면 빈 DB가 만들어지고 스키마까지 맞춰진다.
        .create_if_missing(true)
        // [핵심] WAL(Write-Ahead Logging). 기본 저널 모드에서는 쓰기가 읽기를 통째로
        // 막지만, WAL에서는 **읽기와 쓰기가 동시에** 진행된다. 웹 서버처럼 읽기 요청이
        // 섞여 들어오는 곳에서는 사실상 필수다.
        //
        // [주의] WAL은 DB 파일 옆에 `-wal`, `-shm` 파일을 함께 만든다. 백업할 때 본
        // 파일만 복사하면 최근 쓰기를 잃을 수 있다 — 셋을 함께 다루거나
        // `VACUUM INTO`로 일관된 사본을 뜰 것.
        .journal_mode(SqliteJournalMode::Wal)
        // WAL과 짝을 이루는 설정. FULL은 커밋마다 fsync하지만, WAL + NORMAL이면
        // 체크포인트 시점에만 fsync하면서도 "프로세스가 죽어도 커밋된 데이터는 남는다"가
        // 유지된다(OS 자체가 죽는 경우에만 마지막 트랜잭션을 잃을 수 있다).
        .synchronous(SqliteSynchronous::Normal)
        // [주의] SQLite는 외래키 제약을 **기본으로 강제하지 않는다.** 커넥션마다 켜줘야
        // 하며, 안 켜면 스키마에 적어둔 references가 조용히 아무 일도 하지 않는다.
        .foreign_keys(true)
        // [설명] Postgres 버전의 statement_timeout을 대체하는 자리이지만 성격이 다르다.
        // statement_timeout은 "쿼리가 오래 걸리면 끊는다"였고, busy_timeout은 "다른
        // 커넥션이 쥔 쓰기 락을 이만큼 기다려본다"이다. SQLite에는 실행 시간 자체를
        // 제한하는 설정이 없으므로, 그 그물은 routes/의 요청 타임아웃이 대신 친다.
        .busy_timeout(Duration::from_secs(cfg.db_busy_timeout_secs));

    // [주의] Postgres에서는 max_connections를 늘리면 동시 처리량이 따라 올랐지만
    // SQLite는 그렇지 않다 — **쓰기는 언제나 한 번에 하나**다(WAL이어도 마찬가지). 이
    // 값은 "동시에 읽을 수 있는 수"에 가깝고, 무작정 키우면 쓰기 락 경합만 늘어난다.
    SqlitePoolOptions::new()
        .max_connections(cfg.db_max_connections)
        .acquire_timeout(Duration::from_secs(cfg.db_acquire_timeout_secs))
        .connect_with(options)
        .await
}

/// 이 서비스의 모든 저장소 trait을 한 몸에 구현하는 SQLite 어댑터.
// [설명] trait은 영역별로 나눠져 있지만 구현체는 하나다 — 커넥션 풀이 하나뿐이고, 영역이
// 달라도 같은 트랜잭션 경계/설정을 공유하기 때문. 소비자 입장에서는 여전히 자기 영역의
// trait 하나만 보이므로, 구현이 뭉쳐 있다는 사실이 위로 새어나가지 않는다.
pub struct SqliteStore {
    pub(crate) pool: SqlitePool,
}

impl SqliteStore {
    // SqlitePool 자체가 내부적으로 Arc라 clone이 저렴하므로, 소유권을 받아 들고 있어도 부담이 없다.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
