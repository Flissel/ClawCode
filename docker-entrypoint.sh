#!/bin/sh
# Read Docker secrets into environment variables.
# In Swarm mode, secrets are files at /run/secrets/<name>.
# In Compose mode, they're also mounted there via the secrets: directive.

for secret_file in /run/secrets/*; do
    if [ -f "$secret_file" ]; then
        secret_name=$(basename "$secret_file")
        # Convert secret name to uppercase env var
        # e.g. openrouter_api_key -> OPENROUTER_API_KEY
        env_var=$(echo "$secret_name" | tr '[:lower:]' '[:upper:]')
        # Only set if not already set (env vars take precedence)
        if [ -z "$(eval echo \$$env_var)" ]; then
            export "$env_var=$(cat $secret_file)"
            echo "[entrypoint] Loaded secret: $secret_name -> $env_var"
        fi
    fi
done

# Also check for _FILE suffixed env vars (Docker convention)
# e.g. OPENROUTER_API_KEY_FILE=/run/secrets/openrouter_api_key
for var in $(env | grep '_FILE=' | cut -d= -f1); do
    base_var=$(echo "$var" | sed 's/_FILE$//')
    file_path=$(eval echo \$$var)
    if [ -f "$file_path" ] && [ -z "$(eval echo \$$base_var)" ]; then
        export "$base_var=$(cat $file_path)"
        echo "[entrypoint] Loaded file secret: $var -> $base_var"
    fi
done

exec "$@"
