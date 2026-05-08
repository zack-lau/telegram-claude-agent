FROM rust:1.78-slim AS builder

WORKDIR /build

# Cache dependency compilation
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs && cargo build --release && rm -rf src

COPY src ./src
# Touch main.rs to force recompile
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false aegis
USER aegis

COPY --from=builder /build/target/release/aegis-api /usr/local/bin/aegis-api

EXPOSE 8080

# Healthcheck is handled by the ALB target group (GET /health → 200).
# No in-container curl needed.

ENTRYPOINT ["/usr/local/bin/aegis-api"]
