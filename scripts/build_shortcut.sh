#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHORTCUT_JSON="${ROOT_DIR}/shortcuts/叫下戴维斯.shortcut.json"
TMP_WFLOW="${ROOT_DIR}/shortcuts/叫下戴维斯.wflow"
OUTPUT_SHORTCUT="${ROOT_DIR}/shortcuts/叫下戴维斯.shortcut"

if ! command -v plutil >/dev/null 2>&1; then
  echo "plutil is required to build the shortcut template." >&2
  exit 1
fi

if ! command -v shortcuts >/dev/null 2>&1; then
  echo "shortcuts CLI is required to sign the shortcut template." >&2
  exit 1
fi

plutil -convert binary1 "${SHORTCUT_JSON}" -o "${TMP_WFLOW}"
shortcuts sign -m anyone -i "${TMP_WFLOW}" -o "${OUTPUT_SHORTCUT}"
rm -f "${TMP_WFLOW}"

echo "Built ${OUTPUT_SHORTCUT}"
