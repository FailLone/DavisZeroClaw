#!/usr/bin/env bash

TESTS_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${TESTS_SCRIPT_DIR}/../.." && pwd)"
RUNTIME_DIR="${REPO_ROOT}/.runtime/davis"

fail() {
  echo "❌ $*" >&2
  exit 1
}

info() {
  echo "ℹ️ $*"
}

pass() {
  echo "✅ $*"
}
