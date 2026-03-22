# plcc compiler image — build once, reuse for all ST compilations
FROM rust:1.83-bookworm AS builder

RUN apt-get update && apt-get install -y \
    wget lsb-release software-properties-common gnupg \
    && wget -q https://apt.llvm.org/llvm.sh && chmod +x llvm.sh \
    && ./llvm.sh 21 && apt-get install -y llvm-21-dev \
    && rm -rf /var/lib/apt/lists/* /llvm.sh

ENV LLVM_SYS_211_PREFIX=/usr/lib/llvm-21

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release -p plcc

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    gcc-arm-none-eabi libllvm21 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/plcc /usr/local/bin/plcc

ENTRYPOINT ["plcc"]
