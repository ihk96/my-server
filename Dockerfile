# [중요] 이 Dockerfile은 **이미지를 만들기 위한 것이 아니다.** 배포 서버(Oracle Linux 9 /
# aarch64)에는 도커를 두지 않고 바이너리만 올리며, 여기서는 그 바이너리를 만들어 꺼내는
# 일만 한다. 즉 "빌드 도구로서의 도커"다.
#
# 이렇게 하는 이유는 크로스 컴파일 툴체인(aarch64 타깃, zig)을 개발 머신에 설치하지
# 않기 위해서다. 필요한 것이 전부 이 이미지 안에만 존재하므로 개발 환경이 깨끗하게 남고,
# 누가 빌드하든 같은 조건이 된다.
#
# 사용법:
#   docker buildx build --output type=local,dest=./dist .
#   file ./dist/server      # ARM aarch64 / statically linked 인지 반드시 확인
#
# 마지막 스테이지가 scratch라서 이미지가 만들어지지 않고, --output이 지정한 호스트
# 디렉토리에 산출물만 떨어진다. 컨테이너를 띄워 docker cp로 꺼낼 필요가 없다.

# ---------------------------------------------------------------------------
# builder
# ---------------------------------------------------------------------------
# [설명] 베이스는 x86_64 그대로 둔다. 예전에는 최종 이미지의 debian:bookworm-slim과
# glibc 버전을 맞추려고 bookworm을 골랐지만, 이제 산출물이 musl 정적 링크라서 베이스의
# glibc가 결과물에 전혀 반영되지 않는다. 베이스는 그냥 "컴파일러가 도는 곳"일 뿐이다.
#
# [설계] arm64 베이스(--platform=linux/arm64)를 써서 에뮬레이션으로 네이티브 빌드하는
# 방법도 있지만, QEMU 위에서 도는 Rust 풀빌드는 매우 느리다(릴리즈 프로필의 lto = true가
# 그걸 더 키운다). x86_64에서 크로스 컴파일하는 편이 압도적으로 빠르다.
FROM rust:1-bookworm AS builder

WORKDIR /app

# [핵심] 배포 대상이 Oracle Linux 9 / aarch64인데 왜 gnu가 아니라 musl인가.
#
# 동적 링크(-gnu) 바이너리는 빌드 환경의 glibc 버전을 요구한다. 이 베이스(bookworm)는
# glibc 2.36이고 OL9는 2.34라서 버전이 어긋나며, 개발 머신의 WSL(Debian 13, glibc 2.41)에서
# 직접 빌드하면 격차가 더 벌어진다. 베이스를 배포판마다 맞춰 고르는 방법도 있지만,
# 그러면 "서버를 업그레이드하면 빌드도 따라 고쳐야 하는" 결합이 생긴다.
#
# musl로 정적 링크하면 libc가 바이너리 안에 들어가므로 배포판/glibc 버전과 무관해진다.
# 서버에는 런타임 의존성을 아무것도 설치하지 않아도 된다 — 파일 하나만 올리면 끝이다.
#
# [주의] musl의 기본 allocator는 멀티스레드 부하에서 glibc보다 느리다. 지금 규모에서는
# 무시해도 되고, 문제가 되면 mimalloc을 global allocator로 붙이는 것이 첫 대응이다.
RUN rustup target add aarch64-unknown-linux-musl

# [핵심] 크로스 컴파일러로 zig를 쓴다. 링커만 필요했다면 rustup이 함께 주는 rust-lld로
# 충분했겠지만, 이 프로젝트는 **C 코드를 aarch64용으로 컴파일**해야 한다 — sqlx의 sqlite
# 드라이버가 libsqlite3-sys를 통해 SQLite amalgamation(C)을 함께 빌드하기 때문이다.
# (Postgres 드라이버를 쓸 때는 순수 Rust라 이 문제가 없었다. DB를 바꾸면서 생긴 요구다.)
#
# [설계] 후보가 몇 있었지만 zig가 맞았다.
#   - rust-lld    : 링커일 뿐이라 C를 컴파일하지 못한다. 탈락.
#   - clang       : --target으로 aarch64를 겨눌 수는 있으나 musl **헤더**가 없다.
#                   rustup이 주는 것은 libc.a와 crt 오브젝트뿐이고 헤더는 포함되지 않는다.
#   - gcc-aarch64-linux-gnu : glibc용이라 musl 타깃과 결이 어긋난다.
#   - zig         : 여러 타깃의 musl 헤더와 libc를 **자기 안에 담고 있어서**, 이거 하나로
#                   컴파일과 링킹이 모두 성립한다. cargo-zigbuild가 그 zig를 cc/링커로
#                   꽂아주는 얇은 래퍼다.
#
# [주의] 버전을 고정한다. 빌드 결과가 "그날 zig 최신판이 무엇이었나"에 따라 달라지지 않게
# 하려는 것이고, zig는 아직 1.0 이전이라 릴리즈마다 인터페이스가 흔들린다. 올릴 때는
# cargo-zigbuild가 그 버전을 지원하는지 함께 확인할 것.
#
# [주의] tarball 이름 규칙이 0.14.1에서 바뀌었다. 그 전은 zig-linux-x86_64-<ver>이고
# 그 이후는 zig-x86_64-linux-<ver>다(OS와 아키텍처 순서가 뒤집혔다). 버전을 내릴 일이
# 있으면 아래 URL의 파일명도 함께 고쳐야 한다 — 안 그러면 404가 난다.
ARG ZIG_VERSION=0.14.1

RUN curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-x86_64-linux-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz \
    && mkdir -p /opt/zig \
    && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1 \
    && rm /tmp/zig.tar.xz \
    && ln -s /opt/zig/zig /usr/local/bin/zig

# [설명] cargo zigbuild 서브커맨드를 추가한다. 아래 빌드가 `cargo build`가 아니라
# `cargo zigbuild`인 이유가 이것이다 — 하는 일은 같고, CC와 링커를 zig로 바꿔 끼운다.
RUN cargo install cargo-zigbuild --locked

# [설명] --target을 매번 붙이는 대신 환경변수로 고정한다. 아래 빌드가 두 번 나오는데
# (더미 빌드 / 실제 빌드), 한쪽에만 --target을 붙이는 사고가 나면 더미는 x86_64로
# 컴파일돼서 의존성 캐시가 통째로 무의미해진다. 한 곳에서 정하면 그럴 일이 없다.
#
# [주의] 이 줄은 반드시 위 `cargo install`보다 **뒤에** 있어야 한다. CARGO_BUILD_TARGET은
# cargo가 실행하는 모든 빌드에 적용되므로, 앞에 두면 cargo-zigbuild 자신까지 aarch64로
# 빌드하려 들고 링크 단계에서 깨진다 — 그건 이 컨테이너(x86_64)에서 도는 도구다.
ENV CARGO_BUILD_TARGET=aarch64-unknown-linux-musl

# 의존성 캐시 레이어. 매니페스트만 먼저 복사하고 "내용이 빈 소스"로 한 번 빌드해서,
# axum/sqlx/tokio 같은 외부 크레이트의 컴파일 결과를 이 레이어에 굳혀둔다(SQLite의 C
# 컴파일도 여기서 끝난다). 이후 우리 소스만 바뀌면 Docker는 Cargo.toml/Cargo.lock이
# 그대로인 걸 보고 이 레이어를 재사용하므로, 재빌드가 "우리 크레이트 하나"로 줄어든다.
# 풀빌드는 릴리즈 프로필의 lto = true 때문에 수 분 단위로 걸린다(거슬리면 lto = "thin"이
# 첫 손볼 지점).
COPY Cargo.toml Cargo.lock ./

# [핵심] .cargo/config.toml에는 SQLX_OFFLINE=true가 들어 있다. sqlx의 query! 계열 매크로는
# 기본적으로 컴파일 타임에 DATABASE_URL로 실제 DB에 접속해 SQL을 검증하는데, 빌드 컨테이너에는
# 그런 DB가 없다(있어서도 안 된다 — 빌드가 특정 개발 머신의 DB에 의존하게 되므로).
# 이 설정이 있어야 매크로가 대신 아래에서 복사하는 .sqlx/ 캐시를 보고 검증한다.
#
# [설계] 같은 값을 여기서 ENV SQLX_OFFLINE=true로 박을 수도 있다. 다만 그러면 동일한 설정이
# 두 군데에 존재해 한쪽만 고치는 사고가 나므로, "이 도커 빌드의 사정"이 아니라 "프로젝트의
# 성질"에 해당하는 값은 프로젝트가 이미 가진 설정 파일을 그대로 가져오는 쪽을 택했다.
COPY .cargo ./.cargo

# Cargo는 src/bin/ 아래 파일 이름으로 바이너리를 인식하고, Cargo.toml의
# default-run = "server"도 그 이름이 실제로 존재할 것을 요구한다. 그래서 더미도
# 실제 bin 구성과 같은 이름이어야 이 단계가 통과한다.
#
# [주의] 여기도 `cargo build`가 아니라 `cargo zigbuild`여야 한다. 이 단계에서 이미
# libsqlite3-sys의 C 컴파일이 일어나므로, 평범한 cargo build로는 여기서 먼저 깨진다.
RUN mkdir -p src/bin \
    && echo 'fn main() {}' > src/bin/server.rs \
    && touch src/lib.rs \
    && cargo zigbuild --release \
    && rm -rf src

# migrations/도 반드시 함께 들어와야 한다 — server/mod.rs의 `sqlx::migrate!()`는
# 런타임이 아니라 **컴파일 타임**에 이 디렉토리를 읽어 SQL을 바이너리에 내장시키는
# 매크로다. 덕분에 배포 서버에는 migrations/를 올리지 않아도 동작한다.
COPY src ./src
COPY migrations ./migrations

# `cargo sqlx prepare`가 만들어 저장소에 커밋해 둔 쿼리 검증 캐시. 각 쿼리의 SQL 문자열과
# 그때 DB가 알려준 컬럼 타입/NULL 허용 여부가 JSON으로 들어 있어서, 매크로가 DB 없이도
# 같은 검증을 수행할 수 있다.
#
# [주의] 이 디렉토리가 없거나 낡았으면 여기서 빌드가 멈춘다. README의 "SQL을 고칠 때"를
# 먼저 거칠 것.
#
# [주의] SQL을 고칠 때마다 갱신해야 한다(`cargo sqlx prepare -- --all-targets`). 갱신을
# 빠뜨린 채 쿼리를 바꾸면 캐시에 없는 쿼리라며 여기서 빌드가 실패하므로 대개 바로 드러난다.
# 다만 SQL은 그대로인데 마이그레이션으로 컬럼 타입만 바뀐 경우는 빌드가 통과하고 런타임에
# 터지므로, 마이그레이션을 추가했다면 쿼리를 안 고쳤더라도 prepare를 다시 돌릴 것.
COPY .sqlx ./.sqlx

# [주의] 캐시 레이어 트릭의 유일한 함정. 위 더미 빌드의 산출물을 지우고 진짜 빌드를 한다.
# 더미와 실제 소스의 크레이트 이름이 같아서, 지우지 않으면 Cargo가 이미 빌드된 것으로
# 착각해 **빈 main()이 든 바이너리가 그대로 산출물로 나온다.** 빌드는 성공하고 서버에서야
# 이상하다는 걸 알게 되는 종류라 제일 위험하다 — README의 산출물 검증을 거르지 말 것.
#
# [주의] 아래 my_server는 Cargo.toml의 package name(하이픈은 밑줄로 바뀐다)이다.
# 프로젝트 이름을 바꿨다면 여기도 함께 고쳐야 한다. fingerprint까지 지우는 것은
# 바이너리만 지웠을 때 Cargo가 lib 쪽을 최신으로 판단해 넘어가는 경우를 막기 위해서다.
#
# [설명] 경로에 타깃 이름이 낀다. --target을 주면 산출물이 target/release/가 아니라
# target/<타깃>/release/ 아래로 들어가기 때문이다.
RUN rm -f target/aarch64-unknown-linux-musl/release/server \
    && rm -f target/aarch64-unknown-linux-musl/release/deps/server-* \
    && rm -rf target/aarch64-unknown-linux-musl/release/.fingerprint/my_server-* \
    && cargo zigbuild --release

# ---------------------------------------------------------------------------
# export
# ---------------------------------------------------------------------------
# [설명] 예전의 runtime 스테이지(debian:bookworm-slim + appuser + EXPOSE + CMD)를 대체한다.
# 컨테이너로 실행할 일이 없으므로 베이스도 실행 계정도 진입점도 필요 없다. scratch는
# 아무것도 들어 있지 않은 빈 스테이지이고, --output과 함께 쓰면 여기 COPY한 것이 그대로
# 호스트 디렉토리에 떨어진다.
#
# [주의] 도커가 해주던 일 중 배포 쪽으로 옮겨간 것이 두 가지 있다. README의 "서버에서
# 실행하기"에서 systemd unit으로 되살린다.
#   - ENV RUST_LOG=info   빠뜨리면 로그가 아예 나오지 않는다
#   - USER appuser        루트로 서비스를 돌리지 않기 위한 전용 계정
FROM scratch AS export

COPY --from=builder /app/target/aarch64-unknown-linux-musl/release/server /server
