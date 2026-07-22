#!/usr/bin/env bash
set -uo pipefail

alex_bin="${ALEX_BIN:-alex}"
fixture_dir="${1:-crates/alex-fakeprov/fixtures}"
providers=(anthropic openai gemini xai kimi openrouter cliproxyapi exo amp)
status_json="$("$alex_bin" status --json)" || {
  echo "could not read daemon/account status; is alex daemon running?" >&2
  exit 1
}

has_account() {
  local provider="$1"
  STATUS_JSON="$status_json" PROVIDER="$provider" python3 - <<'PY'
import json
import os
import sys

document = json.loads(os.environ["STATUS_JSON"])
accounts = document.get("accounts", [])
provider = os.environ["PROVIDER"]
sys.exit(0 if any(account.get("provider") == provider for account in accounts) else 1)
PY
}

fixture_provider() {
  case "$1" in
    openai) echo openai-api ;;
    gemini) echo gemini-api ;;
    xai) echo grok ;;
    *) echo "$1" ;;
  esac
}

for provider in "${providers[@]}"; do
  if ! has_account "$provider"; then
    echo "skip $provider: no logged-in account"
    continue
  fi
  since="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "recording $provider surfaces"
  "$alex_bin" ping "$provider" || echo "$provider ping did not complete; exporting any captured response" >&2
  "$alex_bin" limits --json >/dev/null || echo "$provider usage refresh did not complete; exporting any captured response" >&2
  "$alex_bin" provider list >/dev/null || echo "$provider admin provider query did not complete" >&2
  "$alex_bin" fixtures record \
    --provider "$(fixture_provider "$provider")" \
    --since "$since" \
    --limit 100 \
    --out "$fixture_dir" || echo "skip $provider export: no matching captured traces" >&2
done

"$alex_bin" fixtures inventory --dir "$fixture_dir"
