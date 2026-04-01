# ClawCode Router — Multi-backend LLM routing service
# Includes: ClawCode binary, Claude CLI, Kilo CLI
#
# Build: docker build -t flissel/clawcode-router .
# Run:   docker run -p 8090:8090 --env-file .env flissel/clawcode-router

FROM rust:1.94-slim-bookworm AS builder

WORKDIR /build
COPY rust/ rust/
WORKDIR /build/rust
RUN cargo build --release -p clawcode-router -p rusty-claude-cli && \
    strip target/release/clawcode-router target/release/rusty-claude-cli

# --- Runtime ---
FROM node:22-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    ca-certificates \
    git \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI + Kilo Code CLI
RUN npm install -g @anthropic-ai/claude-code@latest @kilocode/cli@latest 2>/dev/null || true

# Copy Rust binaries
COPY --from=builder /build/rust/target/release/clawcode-router /usr/local/bin/clawcode-router
COPY --from=builder /build/rust/target/release/rusty-claude-cli /usr/local/bin/rusty-claude-cli
RUN ln -s /usr/local/bin/rusty-claude-cli /usr/local/bin/clawcode

# Config
WORKDIR /app
COPY clawcode-router.toml /app/clawcode-router.toml

# Secrets helper: read /run/secrets/* into env vars at startup
COPY docker-entrypoint.sh /app/docker-entrypoint.sh
RUN chmod +x /app/docker-entrypoint.sh

EXPOSE 8090

HEALTHCHECK --interval=30s --timeout=10s --retries=3 --start-period=15s \
    CMD curl -f http://localhost:8090/health || exit 1

ENTRYPOINT ["/app/docker-entrypoint.sh"]
CMD ["clawcode-router", "--config", "/app/clawcode-router.toml"]
