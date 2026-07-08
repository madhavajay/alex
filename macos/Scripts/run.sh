#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
CONFIGURATION="${CONFIGURATION:-debug}" ./Scripts/package_app.sh
pkill -x AlexandriaBar 2>/dev/null || true
open "dist/AlexandriaBar.app"
