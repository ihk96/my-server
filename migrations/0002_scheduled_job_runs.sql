-- 스케줄러의 실행 기록 겸 중복 실행 방지 락. 예제가 아니라 골격이므로 지우지 않는다
-- (스케줄러 자체를 쓰지 않기로 했다면 src/scheduler/와 함께 지운다).
--
-- [아키텍처] 이 테이블 하나가 "같은 발화는 한 번만 실행된다"를 만든다. 발화 시각
-- (scheduled_for)은 크론식으로 계산하므로 언제 계산하든 정확히 같은 값이 나온다. 그 값을
-- 기본키로 삼아 INSERT에 성공한 쪽만 job을 실행하고, 실패한 쪽은 충돌을 받고 조용히
-- 넘어간다 — 별도 락 서비스가 필요 없다.
--
-- [설명] 원래 이 구조는 "인스턴스가 여러 개여도 한 번만"을 노린 것이었다. SQLite는 파일
-- 기반이라 사실상 단일 인스턴스이므로 그 목적은 사라졌지만, 이 락은 **재기동 시 이미
-- 실행한 발화를 다시 돌리는 것**을 여전히 막아준다(기록이 남아 있으므로). 그래서 남긴다.
--
-- [설명] 시각은 0001과 같은 이유로 text(ISO 8601)다. 문자열 정렬이 시간 정렬과 일치하므로
-- 아래 인덱스와 정리 job의 범위 조건(scheduled_for < ?)이 의도대로 동작한다.
create table scheduled_job_runs (
    -- scheduler::Job의 name. 이 값이 락의 키이므로 job 이름은 서로 겹치면 안 된다
    -- (Scheduler::build가 기동 시점에 중복을 걸러낸다).
    job_name      text not null,
    -- 크론식이 가리키는 발화 시각. "언제 실행됐는가"(started_at)가 아니라
    -- "언제 실행됐어야 하는가"다. 둘을 비교하면 스케줄러가 밀렸는지 알 수 있다.
    scheduled_for text not null,
    started_at    text not null,
    -- 아래 둘은 실행이 끝나야 채워진다. finished_at이 null인 채로 오래 남아 있는 행은
    -- "실행 도중 프로세스가 죽었다"는 뜻이다.
    finished_at   text,
    -- 실패했을 때의 에러 문자열. 성공이면 null.
    error         text,
    primary key (job_name, scheduled_for)
);

-- [설명] 이 테이블은 발화할 때마다 한 줄씩 쌓인다(1분 주기 job 하나면 하루 1440행). 그래서
-- 오래된 행을 지우는 job이 domain/jobs.rs에 함께 등록돼 있다 — 스케줄러가 자기 기록을 자기가
-- 청소하는 셈이다. 보관 기간은 그 등록 줄에서 정한다.

-- [설명] 그 정리 job과 "최근 실행 조회"를 위한 인덱스. 락 자체는 기본키만으로 동작하므로,
-- 정리를 하지 않는다면 지워도 된다.
create index scheduled_job_runs_scheduled_for_idx on scheduled_job_runs (scheduled_for desc);
