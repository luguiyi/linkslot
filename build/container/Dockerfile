# linkSlot 构建镜像（P2-08）
#
# 目标：把"设备构建所需的一切"固化下来，本地与 CI 用同一镜像。
# 设备是 Debian 11 bullseye（glibc 2.31），因此基座必须是 bullseye。
#
# 用法：
#   docker build -t ghcr.io/luguiyi/linkslot-build:1.0.0 build/container
#   docker push ghcr.io/luguiyi/linkslot-build:1.0.0
#   # 发布前把下面的 FROM 改成带 digest 的写法（pin）：
#   # FROM rust:1.96-bullseye@sha256:<digest>

FROM rust:1.96-bullseye

# 构建工具链：C/C++（多个核心）、cmake（mGBA）、zip/file（打包与校验）、
# python3（manifest/package 脚本）、git/curl（取源码）。
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        cmake \
        curl \
        file \
        git \
        python3 \
        unzip \
        zip \
    && rm -rf /var/lib/apt/lists/*

# 设备是 bullseye；显式声明，避免基础镜像升级后静默漂移。
ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_TERM_COLOR=always

# 预取标准库，缩短首次构建时间（rust-toolchain.toml 会再次校验版本）。
RUN rustup component add rustfmt clippy \
    && rustc --version \
    && cargo --version

WORKDIR /src
