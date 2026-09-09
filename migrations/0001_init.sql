-- 예제 테이블. domain/example.rs, store/example.rs와 짝이며 지울 대상이다.
--
-- [설명] SQLite에는 uuid도 timestamptz도 없다. 억지로 흉내내지 않고 둘 다 text에 담되,
-- **값을 만드는 일은 Rust 쪽으로 옮긴다**(store/example.rs의 insert_example 참고).
-- Rust 쪽 타입도 uuid::Uuid가 아니라 String이다 — 아래 id 컬럼 주석 참고.
-- Postgres 버전은 gen_random_uuid()와 now()를 컬럼 기본값으로 뒀지만, SQLite에는
-- uuid 생성 함수가 아예 없고 시각도 CURRENT_TIMESTAMP가 초 단위 'YYYY-MM-DD HH:MM:SS'
-- 문자열이라 chrono가 기대하는 형식과 어긋난다. 기본값에 기대는 대신 애플리케이션이
-- 명시적으로 넣는 편이 형식이 한 곳에서 결정되므로 덜 헷갈린다.
--
-- [설명] 시각을 text(ISO 8601)로 담는 것은 SQLite의 권장 방식 중 하나이고, 이 형식은
-- **문자열 정렬이 곧 시간 정렬**이라 order by / 범위 조건이 그대로 성립한다.

create table users (
    id         text not null primary key,
    login_id    text not null unique,
    password_hash text not null,
    name       text not null,
    created_at text not null,
    last_login_at text
);

create table sessions (
    id         text not null primary key,
    user_id    text not null,
    created_at text not null,
    expires_at text not null,
    last_accessed_at text
);

insert into users (id, login_id, password_hash, name, created_at, last_login_at)
values ("7a1eea9d-bcf4-4b36-8654-faed03d2628b","dlsgur2323@gmail.com","$argon2id$v=19$m=19456,t=2,p=1$nDxra6QCAlHxWI4K65s2eQ$pqscUjpYPvbnlvXxMCk942sab9fqQY3OAMzQOq9O7oI", "김인혁", datetime('now'), null)

