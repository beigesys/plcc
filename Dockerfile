# plcc: IEC 61131-3 Structured Text compiler
# LLVM is statically linked — the binary is self-contained.
#
# Build: docker build -t plcc .
# Use:   docker run plcc compile program.st -o out.o --target thumbv7em-unknown-none-eabihf

FROM rust:1.85-bookworm AS builder

RUN apt-get update && apt-get install -y \
    wget lsb-release software-properties-common gnupg \
    && wget -q https://apt.llvm.org/llvm.sh && chmod +x llvm.sh \
    && ./llvm.sh 21 && apt-get install -y llvm-21-dev libpolly-21-dev \
    && rm -rf /var/lib/apt/lists/* /llvm.sh

ENV LLVM_SYS_211_PREFIX=/usr/lib/llvm-21

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release -p plcc

# Runtime: just the binary. No LLVM needed — it's statically linked.
FROM debian:bookworm-slim
COPY --from=builder /build/target/release/plcc /usr/local/bin/plcc
ENTRYPOINT ["plcc"]
