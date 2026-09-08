# my-server

axum + sqlx + tokio + SQLite 백엔드 서비스. axum-service-skeleton을 복사해 시작했고,
DB를 Postgres에서 SQLite로 바꿨다(자원이 넉넉하지 않은 개인 서버에 올리기 위해 별도 DB
프로세스를 두지 않는다). 포트를 전부 `automock`으로 바꿔 끼우므로 **테스트는 DB 없이
돌고**, 빌드도 커밋된 `.sqlx/` 캐시만 있으면 DB 없이 성립한다.

## 시작하기

```bash
# sqlx-cli는 sqlite feature로 설치해야 한다. postgres 전용으로 깔려 있으면
# `no driver found for URL scheme "sqlite"`가 난다.
cargo install sqlx-cli --no-default-features --features rustls,sqlite

# 개발용 DB는 파일 하나면 끝이다. 서버 프로세스도, 계정도, 포트도 없다.
echo 'DATABASE_URL=sqlite://dev.db' > .env
sqlx database create
sqlx migrate run
```

그리고 `cargo run`. `dev.db`는 `.gitignore`에 들어 있다(WAL 모드라 `-wal`, `-shm`도 함께).

```bash
cargo test             # DB 없이 통과해야 정상
cargo build            # DB 없이 통과해야 정상
cargo run              # 서버 (= cargo run --bin server)
```

`.env`는 gitignore 대상이고 `DATABASE_URL` 한 줄이면 충분하다. sqlx-cli와 `query!` 매크로가
DB에 붙기 위한 것이지 애플리케이션 설정 창구가 아니다 — 배포 시 주입할 값의 목록은
`.env.example`에 있다.

### 쿼리 캐시(`.sqlx/`)가 하는 일

SQL은 sqlx의 `query!` 계열 매크로로 쓰여 있어 **컴파일 타임에 스키마와 대조**된다. 기본
동작은 `DATABASE_URL`로 실제 DB에 붙어 물어보는 것인데, `.cargo/config.toml`의
`SQLX_OFFLINE=true` 때문에 대신 저장소에 커밋된 `.sqlx/` 캐시를 본다.

이 캐시가 곧 **"DB 없이 빌드할 수 있다"의 근거**이고, 그래서 도커 빌드 컨테이너 안에서도
컴파일이 성립한다. 반대로 **캐시가 없거나 낡으면 빌드가 실패한다** — 갱신 방법은 다음 절.

### SQL을 고칠 때

쿼리나 마이그레이션을 건드렸다면 두 단계를 거친다. **순서가 중요하다** — 매크로는
그 시점 DB에 실제로 존재하는 스키마를 보고 타입을 뽑으므로, 마이그레이션이 먼저 적용되어
있어야 한다.

```bash
sqlx migrate run                      # 1. 스키마를 최신으로
cargo sqlx prepare -- --all-targets   # 2. .sqlx/ 캐시 갱신 → 커밋
```

`--all-targets`를 생략하면 lib 타겟만 스캔해서, 테스트나 `src/bin/`에 매크로가 들어갔을 때
그 쿼리가 캐시에서 빠진다.

**놓치기 쉬운 경우가 하나 있다.** SQL 문자열은 그대로인데 마이그레이션으로 컬럼 타입만
바뀌면, 캐시가 낡아도 빌드는 통과하고 런타임에 터진다. 마이그레이션을 추가했다면 쿼리를
안 고쳤더라도 `prepare`를 다시 돌릴 것. CI에서 `cargo sqlx prepare --check`로 강제할 수
있지만, 그러려면 CI에서도 마이그레이션을 적용한 SQLite 파일을 하나 만들어줘야 한다
(Postgres 시절과 달리 서비스 컨테이너를 띄울 필요는 없다 — 파일 하나면 된다).

### SQL은 어디까지 검증되나

매크로가 잡아주는 건 컬럼 이름·타입·NULL 허용 여부, 즉 "SQL이 스키마와 맞는가"까지다.
FK/cascade/unique 같은 **제약의 런타임 동작**은 여전히 실제 DB에 붙여야 확인되며, 그래서
`store/example.rs`의 unique 위반 → `Conflict` 번역 같은 코드가 계속 필요하다.

매크로를 쓸 수 없는 경우도 둘 있다. 인자에 따라 SQL 자체가 달라지는 **동적 쿼리**(조건부
WHERE, 가변 SET 절)는 컴파일 시점에 검증할 문자열이 없으므로 `sqlx::QueryBuilder`로
조립하고, 그 타입에는 `#[derive(sqlx::FromRow)]`를 남긴다. 그리고 `store/health.rs`의
`select 1`처럼 스키마를 참조하지 않는 쿼리는 검증할 것이 없어 그대로 둔다.

**SQLite에서 하나 더.** 컬럼 타입이 다섯 가지(NULL/INTEGER/REAL/TEXT/BLOB)뿐이라, `text`
컬럼을 `DateTime<Utc>`처럼 다른 타입으로 받고 싶으면 select 목록에 `컬럼 as "이름: 타입"`으로
알려줘야 한다. 이 표기는 동시에 "NOT NULL"이라는 선언이기도 하다(nullable이면 `as "이름?: 타입"`).

그리고 **타입을 DB의 결에 맞추는 편이 낫다.** 예제의 `id`가 `Uuid`가 아니라 `String`인 것이
그 예다 — sqlx의 `Uuid`는 SQLite에서 16바이트 BLOB으로만 오가므로, `text` 컬럼에 그 타입을
쓰면 스키마 선언과 실제 저장 형태가 어긋나거나(BLOB이 들어간다) 디코딩이 런타임에 실패한다.

오프라인 설정은 [.cargo/config.toml](.cargo/config.toml)에 있다. 셸 환경변수가 아니라 이
파일에 두는 이유는 rust-analyzer가 띄우는 `cargo check`도 같은 설정을 읽어야 하기 때문이다 —
셸에만 걸면 터미널 빌드는 되는데 에디터에는 매크로마다 에러가 뜬다.

## 빌드 & 배포

배포 서버(Oracle Linux 9 / **aarch64**)에는 도커를 두지 않고 바이너리만 올린다. 대신 **빌드는
컨테이너 안에서** 한다 — 크로스 컴파일 툴체인을 개발 머신에 설치하지 않기 위해서다. 필요한
것이 전부 이미지 안에만 있으므로 개발 환경이 깨끗하게 남고, 누가 빌드하든 같은 조건이 된다.

산출물은 `aarch64-unknown-linux-musl` **정적 링크** 바이너리다. libc가 바이너리 안에 들어가
있어서 배포판이나 glibc 버전과 무관하고, 서버에는 런타임 의존성을 아무것도 설치하지 않아도
된다.

**크로스 컴파일에 zig를 쓴다.** sqlx의 SQLite 드라이버는 `libsqlite3-sys`를 통해 SQLite의
C 소스를 함께 빌드하므로, 링커만으로는 부족하고 **C를 aarch64용으로 컴파일**할 수 있어야
한다. zig는 여러 타깃의 musl 헤더와 libc를 자기 안에 담고 있어서 이거 하나로 컴파일과
링킹이 모두 해결된다. 이유와 다른 후보들은 `Dockerfile` 주석에 적어뒀다.

### 빌드

```bash
docker buildx build --output type=local,dest=./dist .
docker buildx build -o ./dist .                        # 같은 뜻 (경로만 주면 type=local로 해석된다)
```

`--output`은 "빌드 결과를 어디에 어떤 형태로 내보낼 것인가"를 정한다. `type=docker`(로컬
이미지로 등록), `type=registry`(바로 푸시) 같은 선택지 중 `type=local`은 **파일시스템을
호스트 디렉토리에 생파일로 풀어놓는다.** 우리가 원하는 것이 이미지가 아니라 바이너리이므로
이걸 쓴다.

내보내지는 것은 **마지막 스테이지의 파일시스템 전체**다. `Dockerfile`의 마지막 스테이지가
`scratch`(아무것도 들어 있지 않은 빈 스테이지)인 이유가 여기 있다 — 거기 `COPY`한 바이너리
하나뿐이라 `./dist/server` 딱 하나만 떨어진다. 예전처럼 마지막이 `debian:bookworm-slim`이었다면
데비안 rootfs가 통째로 쏟아졌을 것이다.

덤으로 **이미지가 만들어지지 않으므로** 컨테이너를 띄워 `docker cp`로 꺼낼 필요도 없다.

빌드 컨테이너에 DB가 필요 없다. `.cargo/config.toml`과 `.sqlx/`가 함께 복사되어 매크로가
DB 대신 캐시를 보고 검증한다. 바꿔 말하면 **캐시가 낡았다면 이 빌드가 실패한다**
(위 "SQL을 고칠 때" 참고).

`migrations/`는 빌드 컨텍스트에 반드시 있어야 한다 — `sqlx::migrate!()`가 컴파일 타임에
읽어서 SQL을 바이너리에 내장시키기 때문이다. 덕분에 **서버에는 올릴 필요가 없다.**

### 산출물 검증 — 거르지 말 것

빌드 성공이 곧 올바른 산출물을 뜻하지 않는다.

```bash
file ./dist/server
```

기대값: `ELF 64-bit LSB executable, ARM aarch64, ... statically linked`

| 증상 | 원인 |
|---|---|
| `x86-64`로 나옴 | 타깃이 안 먹었다 (`CARGO_BUILD_TARGET`) |
| `dynamically linked` | musl이 아니라 gnu 타깃으로 빌드됐다 |
| 크기가 비정상적으로 작음 | 캐시 레이어의 더미 `fn main() {}`이 그대로 나왔다 |

세 번째가 제일 위험하다. 빌드는 성공하고 서버에서야 이상하다는 걸 알게 된다.
Dockerfile에서 더미 산출물을 지우는 `rm` 줄이 그걸 막는 장치이고, 거기 적힌 크레이트
이름은 `Cargo.toml`의 package name과 같아야 한다.

### 재빌드가 느리다면

소스 한 줄만 고쳐도 레이어 캐시 경계 때문에 우리 크레이트는 매번 재컴파일된다(의존성은
재컴파일되지 않으니 견딜 만한 수준이다). 정 거슬리면 `RUN --mount=type=cache`로 `target/`과
cargo 레지스트리를 캐시 마운트하면 거의 네이티브급이 된다. 풀빌드 자체가 느린 것은 릴리즈
프로필의 `lto = true` 영향이 크며, `lto = "thin"`이 첫 손볼 지점이다.

### 서버에 올리기

정적 바이너리라 **파일 하나만** 올리면 된다. `migrations/`도 공유 라이브러리도 필요 없다.

| 위치 | 내용 |
|---|---|
| `/opt/my-server/server` | 바이너리 |
| `/etc/my-server/env` | 환경변수. `chmod 600` + 서비스 계정 소유 |
| `/var/lib/my-server/` | **SQLite DB 파일.** systemd의 `StateDirectory=`가 만들어준다 |
| 시스템 계정 | 루트로 돌리지 않기 위한 전용 계정. 도커의 `appuser`에 대응한다 |

[주의] Postgres 때와 달리 **서버에 상태가 생긴다.** DB가 파일이므로 서비스 계정이 그
디렉토리에 쓸 수 있어야 하고(WAL 모드라 `-wal`, `-shm` 파일도 함께 만들어진다), 재배포할 때
그 파일을 건드리지 않아야 하며, 백업 대상이기도 하다. 백업은 본 파일만 복사하면 최근 쓰기를
잃을 수 있으니 `VACUUM INTO`로 일관된 사본을 뜨는 편이 안전하다.

포트 8080은 1024 미만이 아니라 root 권한이 필요 없다.

주입할 값의 목록은 `.env.example`에 있고, 최소한 아래 세 개는 챙긴다.

| 변수 | 비고 |
|---|---|
| `DATABASE_URL` | 유일한 필수값. SQLite라 그냥 파일 경로다(`sqlite:///var/lib/my-server/my-server.db`). 파일이 없으면 서버가 만들고 마이그레이션까지 스스로 돌린다 |
| `RUST_LOG=info` | **빠뜨리면 로그가 아예 나오지 않는다.** `EnvFilter::from_default_env()`는 값이 비면 사실상 아무것도 출력하지 않는다. 도커가 `ENV`로 박아주던 자리다 |
| `SERVER_ADDR` | **`127.0.0.1:8080`으로 둔다.** 앞에 Caddy가 있으므로 앱을 외부에 직접 열 이유가 없다 (기본값은 `0.0.0.0:8080`이라 반드시 덮어써야 한다) |
| `CORS_ALLOWED_ORIGINS` | 브라우저에서 다른 오리진으로 호출할 때만. 비우면 CORS 꺼짐 |
| `RATE_LIMIT_BURST` / `RATE_LIMIT_PER_SECOND` | IP당 순간/지속 허용치. 기본 30 / 5 |

`dotenvy`가 읽는 `.env`는 **현재 작업 디렉토리 기준**이라, systemd는 `WorkingDirectory=`를
지정하지 않으면 `/`에서 실행돼 못 읽는다. `EnvironmentFile=`을 쓰는 쪽이 명확하다.

### systemd로 실행하기

```ini
[Service]
Type=exec
User=myserver
ExecStart=/opt/my-server/server
EnvironmentFile=/etc/my-server/env
Restart=always
RestartSec=5
TimeoutStopSec=20
# DB 파일이 살 곳. systemd가 /var/lib/my-server를 만들고 소유권까지 맞춰준다.
StateDirectory=my-server
```

`TimeoutStopSec`은 앱의 `SHUTDOWN_TIMEOUT_SECS`(기본 10초)보다 넉넉해야 한다. 더 짧으면
graceful shutdown이 끝나기 전에 SIGKILL을 맞아서 `server/shutdown.rs`의 종료 흐름이
무의미해진다. 쿠버네티스의 `terminationGracePeriodSeconds`와 같은 역할이다.

`KillSignal`은 기본이 SIGTERM이라 건드릴 필요 없다 — 코드가 이미 SIGTERM을 받는다. 로그는
stdout으로 나가므로 journald가 알아서 수집한다(`journalctl -u <서비스> -f`).

여력이 되면 `NoNewPrivileges`, `ProtectSystem=strict`, `PrivateTmp` 같은 하드닝 지시자를
추가한다. 컨테이너가 제공하던 격리를 일부 대체하는 역할이다.

[주의] `ProtectSystem=strict`는 파일시스템 전체를 읽기 전용으로 만든다. `StateDirectory=`가
만든 경로는 예외로 열리지만, DB를 다른 곳에 두기로 했다면 `ReadWritePaths=`로 따로 열어줘야
한다 — 안 그러면 기동은 되는데 첫 쓰기에서 실패한다.

### 재배포

실행 중인 바이너리를 `cp`로 덮어쓰면 `ETXTBSY`로 실패한다. **임시 이름으로 업로드 →
`mv`로 원자적 교체 → `systemctl restart`** 순서로 한다.

바꾸는 것은 `/opt/my-server/server` 하나뿐이다. `/var/lib/my-server/`의 DB는 그대로 두고,
스키마 변경이 있었다면 새 바이너리가 기동하면서 마이그레이션을 알아서 적용한다.

### 앞단 Caddy와의 역할 분담

TLS는 Caddy가 맡고(Let's Encrypt 자동 발급), 앱은 평문 HTTP로 루프백에만 붙는다.
어느 쪽이 무엇을 맡는지 정리하면:

| 항목 | 담당 | 비고 |
|---|---|---|
| TLS, 인증서 갱신 | Caddy | 앱은 평문 HTTP |
| 보안 헤더(HSTS 등) | Caddy | |
| 요청 바디 크기 제한 | Caddy | `request_body max_size` |
| CORS | **앱** | 어떤 오리진에 무엇을 허용하는지는 API가 아는 지식이다. 프록시에서 하면 preflight/Vary를 손으로 맞춰야 해 틀리기 쉽다 |
| 레이트 리밋 | **앱** | Caddy는 표준 빌드에 레이트 리밋이 없어 플러그인을 넣어 직접 빌드해야 한다. 앱에 두면 엔드포인트별로 다르게 걸 수도 있다 |

Caddyfile은 이 정도면 된다.

```
example.com {
    reverse_proxy 127.0.0.1:8080
}
```

`reverse_proxy`가 `X-Forwarded-For`를 알아서 붙여주고, 앱의 레이트 리밋이 그 값을 클라이언트
IP로 쓴다.

**이 구성의 안전성은 `SERVER_ADDR=127.0.0.1:8080`에 걸려 있다.** 앱이 루프백에만 붙어 있어야
헤더를 붙이는 주체가 Caddy뿐이고, 그래야 그 헤더를 믿을 수 있다. 앱을 `0.0.0.0`으로 열면
누구나 `X-Forwarded-For`를 위조해 레이트 리밋을 우회할 수 있으므로, 그때는
`routes/rate_limit.rs`의 `SmartIpKeyExtractor`를 `PeerIpKeyExtractor`로 되돌려야 한다.

방화벽에서도 **443만 열면 된다** — 8080은 외부에 노출할 필요가 없다.

### 아직 없는 것 (보안)

- **인증/인가가 없다.** 지금 `/examples`는 누구나 호출할 수 있다. 공개 배포 전에 가장 먼저
  채워야 할 자리다.
- systemd 하드닝 지시자는 위 unit 스케치에 적어둔 수준이고, 실제 unit 파일은 저장소에 없다.
- RPM 등 패키징 설정도 없다. 배포는 바이너리 업로드 + systemd다.

### 접속이 안 될 때

Oracle Cloud는 방화벽이 두 겹이다. 콘솔의 Security List/NSG에서 포트를 열어도 **Oracle Linux
이미지에는 인스턴스 내부 iptables/firewalld 규칙이 기본으로 박혀 있다.** 양쪽 다 확인할 것.
그래도 안 되면 OL9는 SELinux가 기본 enforcing이니 AVC denial을 본다.

## 지울 것

`example`이 붙은 파일들이 예제다. 새 기능을 추가할 때 모양을 그대로 따라 쓰고, 다 쓴 뒤엔 지운다.

```
src/store/example.rs        포트 trait + SQLite 구현
src/domain/example.rs       도메인 로직 (포트를 &dyn으로 받음)
src/domain/example_job.rs   스케줄 job
src/routes/example.rs       HTTP 핸들러
migrations/0001_init.sql    예제 테이블
```

지울 때 함께 손볼 곳: `src/domain/mod.rs`, `src/routes/mod.rs`, `src/store/mod.rs`의 모듈 선언,
`src/state.rs`의 필드, 그리고 `src/domain/jobs.rs`의 등록 항목(파일 자체는 남긴다).

`scheduler/`와 `migrations/0002_scheduled_job_runs.sql`, `src/store/job_run.rs`는 예제가 아니라
골격이다. 배치를 쓰지 않기로 했다면 셋을 함께 지우고 `AppState`의 `job_run_store`와
`server/mod.rs`의 `Scheduler` 두 줄을 걷어낸다.

그리고 **예제를 지운 뒤 `cargo sqlx prepare`를 다시 돌린다.** 캐시에는 지워진 쿼리의
검증 데이터가 그대로 남아 있어서, 갱신하지 않으면 존재하지 않는 테이블의 정보를 계속
커밋하게 된다.

## 설계 규칙

전체 구조와 그 이유는 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)에 있다. 다섯 줄로 줄이면:

1. **로직은 lib, bin은 껍데기.** bin 안의 코드는 다른 bin도 통합 테스트도 재사용할 수 없다.
2. **모듈은 "다른 프로젝트로 옮겼을 때 내용이 그대로인가"로 가른다.** 그대로면 골격, 아니면 `domain/`.
3. **`AppState`는 `Clone` 없이 `new() -> Arc<Self>`.** 공유는 항상 Arc clone.
4. **백그라운드 진입점은 `Arc<AppState>`, 로직은 좁은 `&dyn` 포트.** 스케줄 job도 태스크도 같다.
5. **포트는 trait + `automock`.** 그래서 DB 없이 테스트가 돈다.

## 들어있지 않은 것

의도적으로 뺐다. 필요할 때 넣는 게 낫다고 본 것들:

- **인증** — API key / JWT / OIDC 중 무엇이냐에 따라 모양이 완전히 달라진다. axum의
  `FromRequestParts`로 extractor를 만들어 "검증된 상태"를 타입으로 표현하는 패턴을 쓸 것.
- **아웃바운드 HTTP** — reqwest를 포트(`trait Upstream`) 뒤에 두고 어댑터로 감싼다.
  `store/`와 구조가 같다.
- **캐시 / 이벤트** — 폴더 이름은 기술이 아니라 포트 이름으로 (`redis/`가 아니라 `cache/`,
  `kafka/`가 아니라 `events/`). 단 kafka는 프로듀서가 아웃바운드, 컨슈머는 인바운드라
  한 폴더에 섞으면 안 된다 — 컨슈머는 `server/` 옆에 `consumer/`로 둔다.
- **`/metrics`, `/version`** — 운영용 엔드포인트. 노출 범위 때문에 별도 포트(9090 등)에
  올리는 경우가 많아서, 그 결정과 함께 넣는 게 낫다.
- **`tests/` 통합 테스트** — `server::run`이 lib에 있으므로 가능하다. SQLite라 진입 장벽도
  낮다(임시 파일이나 `sqlite::memory:` 하나면 된다 — Postgres 시절처럼 서버를 띄울 필요가
  없다). 지금은 모든 테스트가 lib 안의 단위 테스트이고, DB 없이 돈다.
- **공용 테스트 헬퍼(`test_support`)** — 각 테스트가 필요한 mock을 직접 만들고 `AppState`를
  직접 조립한다. 포트가 5~6개를 넘어가면 "관심 없는 필드 채우기"가 반복되므로 그때 빌더를
  뽑아내는 게 낫다 — `#[cfg(test)] pub mod`으로 두면 크레이트 전체가 공유할 수 있다.
