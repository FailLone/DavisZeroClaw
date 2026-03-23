#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DAVIS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUNTIME_DIR="${DAVIS_DIR}/.runtime/davis"

stop_process() {
  local name="$1"
  local pid_file="$2"

  if [ ! -f "${pid_file}" ]; then
    echo "ℹ️ ${name} 未运行。"
    return 0
  fi

  local pid
  pid="$(cat "${pid_file}")"

  if kill -0 "${pid}" 2>/dev/null; then
    kill "${pid}"
    echo "✅ 已停止 ${name}，PID: ${pid}"
  else
    echo "ℹ️ ${name} 的 PID 文件已过期，已清理。"
  fi

  rm -f "${pid_file}"
}

echo "======================================"
echo "    停止 DavisZeroClaw 智能管家"
echo "======================================"

stop_process "Channel Server" "${RUNTIME_DIR}/channel.pid"
stop_process "Gateway" "${RUNTIME_DIR}/gateway.pid"
