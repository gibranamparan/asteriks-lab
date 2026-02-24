#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
ENV_FILE="$ROOT_DIR/.env"
AGENT_DIR="$ROOT_DIR/agent"

if [[ ! -f "$ENV_FILE" ]]; then
  echo "[ERROR] Missing .env file at $ENV_FILE"
  exit 1
fi

if [[ ! -d "$AGENT_DIR" ]]; then
  echo "[ERROR] Missing agent directory at $AGENT_DIR"
  exit 1
fi

# shellcheck disable=SC1090
set -a
source "$ENV_FILE"
set +a

required_vars=(
  HEADEND_URL
  HEADEND_AMQP_EXCHANGE
  HEADEND_AMQP_EXCHANGE_TYPE
  HEADEND_AMQP_QUEUE
  HEADEND_AMQP_ROUTING_KEY
  HEADEND_AMQP_USERNAME
  HEADEND_AMQP_PASSWORD
  PJSIP_BASE_DIR
)

for v in "${required_vars[@]}"; do
  if [[ -z "${!v:-}" ]]; then
    echo "[ERROR] Required variable '$v' is missing or empty in .env"
    exit 1
  fi
done

if ! command -v docker >/dev/null 2>&1; then
  echo "[ERROR] docker CLI not found in PATH"
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "[ERROR] Cannot reach Docker daemon. Ensure your user has docker access."
  exit 1
fi

if [[ ! -d "$PJSIP_BASE_DIR" ]]; then
  echo "[ERROR] PJSIP_BASE_DIR '$PJSIP_BASE_DIR' does not exist or is not a directory"
  exit 1
fi

mkdir -p "$PJSIP_BASE_DIR/backups"

echo "[INFO] Running Rust agent locally with env from $ENV_FILE"
echo "[INFO] HEADEND_URL=$HEADEND_URL"
echo "[INFO] PJSIP_BASE_DIR=$PJSIP_BASE_DIR"

cd "$AGENT_DIR"
exec cargo run
