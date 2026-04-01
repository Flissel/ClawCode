#!/bin/sh
# ClawCode Router Entrypoint
# Handles auth for all 3 backends: Claude, Kilo, ClawCode

echo "[entrypoint] ClawCode Router starting..."

# ============================================================
# 1. Docker Secrets → Env Vars
# ============================================================
for secret_file in /run/secrets/*; do
    if [ -f "$secret_file" ]; then
        secret_name=$(basename "$secret_file")
        env_var=$(echo "$secret_name" | tr '[:lower:]' '[:upper:]')
        if [ -z "$(eval echo \$$env_var)" ]; then
            export "$env_var=$(cat $secret_file)"
            echo "[entrypoint] Secret loaded: $secret_name -> $env_var"
        fi
    fi
done

# _FILE suffixed env vars (Docker convention)
for var in $(env | grep '_FILE=' | cut -d= -f1); do
    base_var=$(echo "$var" | sed 's/_FILE$//')
    file_path=$(eval echo \$$var)
    if [ -f "$file_path" ] && [ -z "$(eval echo \$$base_var)" ]; then
        export "$base_var=$(cat $file_path)"
        echo "[entrypoint] File secret loaded: $var -> $base_var"
    fi
done

# ============================================================
# 2. Claude Auth — Fallback Chain
#    Priority: credentials.json (Pro/Max) > ANTHROPIC_API_KEY > skip
# ============================================================
echo "[entrypoint] Claude auth check..."
CLAUDE_OK=false

# 2a. Check for credentials via Docker secret
if [ -f "/run/secrets/claude_credentials" ]; then
    echo "[entrypoint]   Pro/Max credentials found as Docker secret"
    mkdir -p /root/.claude
    cp /run/secrets/claude_credentials /root/.claude/.credentials.json
    chmod 600 /root/.claude/.credentials.json
    echo '{}' > /root/.claude/settings.json
    CLAUDE_OK=true
# 2b. Check for mounted Pro/Max credentials (legacy volume mount)
elif [ -f "/root/.claude/.credentials.json" ]; then
    echo "[entrypoint]   Pro/Max credentials found at /root/.claude/.credentials.json"
    CLAUDE_OK=true
# 2c. Check for API key
elif [ -n "$ANTHROPIC_API_KEY" ]; then
    echo "[entrypoint]   ANTHROPIC_API_KEY set — using API key auth"
    # Create minimal Claude settings so CLI doesn't prompt for login
    mkdir -p /root/.claude
    echo '{}' > /root/.claude/settings.json
    CLAUDE_OK=true
else
    echo "[entrypoint]   No Claude auth found — Claude backend will use fallback"
    echo "[entrypoint]   Mount ~/.claude/ for Pro/Max, or set ANTHROPIC_API_KEY"
fi

# ============================================================
# 3. Kilo Auth — Check OPENAI_API_KEY
# ============================================================
echo "[entrypoint] Kilo auth check..."
KILO_OK=false

if [ -n "$OPENAI_API_KEY" ]; then
    echo "[entrypoint]   OPENAI_API_KEY set — Kilo ready"
    KILO_OK=true
elif [ -f "/root/.kilocode/settings.json" ]; then
    # Check if settings.json references env var that's set
    echo "[entrypoint]   Kilo config found at /root/.kilocode/settings.json"
    KILO_OK=true
else
    echo "[entrypoint]   No Kilo auth found — Kilo backend will use fallback"
    echo "[entrypoint]   Set OPENAI_API_KEY or mount kilo config"
fi

# ============================================================
# 4. ClawCode Auth — Check per provider
# ============================================================
echo "[entrypoint] ClawCode provider check..."

if [ -n "$OPENROUTER_API_KEY" ]; then
    echo "[entrypoint]   OpenRouter: OK"
else
    echo "[entrypoint]   OpenRouter: NO KEY (set OPENROUTER_API_KEY)"
fi

if [ -n "$ANTHROPIC_API_KEY" ]; then
    echo "[entrypoint]   Anthropic: OK"
else
    echo "[entrypoint]   Anthropic: NO KEY"
fi

# Ollama is local, no auth needed
echo "[entrypoint]   Ollama: OK (no auth needed, via host.docker.internal)"

# ============================================================
# 5. Summary
# ============================================================
echo "[entrypoint] ================================"
echo "[entrypoint] Backend Status:"
echo "[entrypoint]   Claude:   $([ "$CLAUDE_OK" = true ] && echo 'READY' || echo 'FALLBACK')"
echo "[entrypoint]   Kilo:     $([ "$KILO_OK" = true ] && echo 'READY' || echo 'FALLBACK')"
echo "[entrypoint]   ClawCode: READY (OpenRouter)"
echo "[entrypoint] ================================"

exec "$@"
