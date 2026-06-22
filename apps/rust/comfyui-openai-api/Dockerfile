# ============================================================
#  Stage 1: Builder — 编译 Rust 二进制
# ============================================================
FROM rust:latest AS builder

# ----- Cargo 国内镜像加速（中科大 USTC）-----
# 使用稀疏协议（sparse）加速索引更新
RUN mkdir -p /usr/local/cargo && \
    { \
      echo '[source.crates-io]'; \
      echo 'replace-with = "ustc"'; \
      echo '[source.ustc]'; \
      echo 'registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"'; \
    } > /usr/local/cargo/config.toml

# ----- 网络与并发优化 -----
ENV CARGO_NET_RETRY=5 \
    CARGO_NET_TIMEOUT=30 \
    CARGO_HTTP_MULTIPLEXING=false \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

# ----- 缓存依赖层（关键加速）-----
# 先只复制 Cargo.toml/lock + 骨架代码，构建并缓存所有依赖
RUN USER=root cargo new --bin comfyui-openai-api
WORKDIR /comfyui-openai-api
COPY apps/rust/comfyui-openai-api/Cargo.toml Cargo.toml
COPY apps/rust/comfyui-openai-api/Cargo.lock Cargo.lock

# 此时只有 hello-world 骨架，构建结果只包含依赖
# Docker 会缓存这一层，后续只要 Cargo.toml/lock 不变就不重跑
RUN cargo build --release
RUN rm -rf src/*.rs

# ----- 编译真实代码 -----
COPY apps/rust/comfyui-openai-api/src src

# 清除旧二进制缓存，强制重新编译真实代码（依赖层仍从缓存取）
RUN rm -f ./target/release/deps/comfyui_openai_api*
RUN cargo install --path .

# （可选）去除调试符号，缩小体积
# RUN strip /usr/local/cargo/bin/comfyui-openai-api

# ============================================================
#  Stage 2: Runtime — 最小运行时镜像
# ============================================================
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl nano && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/local/cargo/bin/comfyui-openai-api ./
COPY workflows /app/workflows

CMD ["./comfyui-openai-api"]
