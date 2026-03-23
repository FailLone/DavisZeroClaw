#!/usr/bin/env bash
set -euo pipefail

# 确保 Homebrew 的二进制文件在 PATH 中 (针对 Apple Silicon 和 Intel Mac)
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

# DavisZeroClaw 启动脚本

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DAVIS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUNTIME_DIR="${DAVIS_DIR}/.runtime/davis"
CONFIG_TEMPLATE="${DAVIS_DIR}/config/davis/config.toml"
CONFIG_DEST="${RUNTIME_DIR}/config.toml"
GATEWAY_LOG="${RUNTIME_DIR}/gateway.log"
CHANNEL_LOG="${RUNTIME_DIR}/channel.log"
GATEWAY_PID_FILE="${RUNTIME_DIR}/gateway.pid"
CHANNEL_PID_FILE="${RUNTIME_DIR}/channel.pid"
ENV_FILE=""

echo "======================================"
echo "    启动 DavisZeroClaw 智能管家"
echo "======================================"

# 1. 检查底层引擎 ZeroClaws 是否安装
if ! command -v zeroclaw >/dev/null 2>&1; then
  echo "❌ 找不到底层引擎 zeroclaw。"
  echo "请先通过 Homebrew 安装: brew install zeroclaw"
  exit 1
fi

# 2. 初始化运行环境
mkdir -p "${RUNTIME_DIR}"
if [ ! -f "${CONFIG_TEMPLATE}" ]; then
  echo "❌ 找不到配置模板: ${CONFIG_TEMPLATE}"
  exit 1
fi

# 加载环境变量 (包含 API Key 等机密信息)
if [ -f "${DAVIS_DIR}/.env.local" ]; then
  ENV_FILE="${DAVIS_DIR}/.env.local"
elif [ -f "${DAVIS_DIR}/.env" ]; then
  ENV_FILE="${DAVIS_DIR}/.env"
fi

if [ -n "${ENV_FILE}" ]; then
  echo "🔑 正在加载 ${ENV_FILE} 环境变量..."
  set -a
  source "${ENV_FILE}"
  set +a
else
  echo "⚠️ 未找到 .env.local 或 .env。你可以先复制 .env.example 为 .env.local 再填写。"
fi

render_runtime_config() {
  if [ -z "${DAVIS_HA_URL:-}" ] || [ -z "${DAVIS_HA_TOKEN:-}" ]; then
    return 1
  fi

  perl -0pe '
    my $ha_url = $ENV{"DAVIS_HA_URL"} // "";
    my $ha_token = $ENV{"DAVIS_HA_TOKEN"} // "";
    s/__DAVIS_HA_URL__/$ha_url/g;
    s/__DAVIS_HA_TOKEN__/$ha_token/g;
  ' "${CONFIG_TEMPLATE}" > "${CONFIG_DEST}"

  chmod 600 "${CONFIG_DEST}" 2>/dev/null || true
}

if render_runtime_config; then
  echo "⚙️ 已根据模板渲染运行时配置: ${CONFIG_DEST}"
elif [ ! -f "${CONFIG_DEST}" ]; then
  echo "❌ 首次启动前，请先在 .env.local 中设置以下变量："
  echo "   OPENROUTER_API_KEY=<你的 OpenRouter API Key>"
  echo "   DAVIS_HA_URL=http://homeassistant.local:8123/api/mcp"
  echo "   DAVIS_HA_TOKEN=<你的 Home Assistant Long-Lived Access Token>"
  echo "   你也可以参考 ${DAVIS_DIR}/.env.example"
  exit 1
else
  echo "ℹ️ 未检测到 DAVIS_HA_URL / DAVIS_HA_TOKEN，继续使用已有运行时配置:"
  echo "   ${CONFIG_DEST}"
fi

export ZEROCLAW_CONFIG_DIR="${RUNTIME_DIR}"
chmod 600 "${CONFIG_DEST}" 2>/dev/null || true

if grep -q '^default_provider = "openrouter"' "${CONFIG_DEST}" && [ -z "${OPENROUTER_API_KEY:-}" ]; then
  echo "❌ 当前默认 provider 为 openrouter，但未检测到 OPENROUTER_API_KEY。"
  echo "   请在 .env.local 或 .env 中设置 OPENROUTER_API_KEY。"
  exit 1
fi

if grep -q 'REPLACE_WITH_YOUR' "${CONFIG_DEST}" || grep -q '__DAVIS_HA_' "${CONFIG_DEST}"; then
  echo "❌ 运行时配置中仍然存在未替换的占位符，请检查 ${CONFIG_DEST}、.env.local 或 .env。"
  exit 1
fi

wait_for_http() {
  local url="$1"
  local attempts="${2:-15}"
  local delay="${3:-1}"
  local i

  for ((i = 0; i < attempts; i++)); do
    if curl -fsS "${url}" >/dev/null 2>&1; then
      return 0
    fi
    sleep "${delay}"
  done

  return 1
}

wait_for_port() {
  local port="$1"
  local attempts="${2:-15}"
  local delay="${3:-1}"
  local i

  for ((i = 0; i < attempts; i++)); do
    if lsof -nP -iTCP:"${port}" -sTCP:LISTEN >/dev/null 2>&1; then
      return 0
    fi
    sleep "${delay}"
  done

  return 1
}

start_process() {
  local name="$1"
  local pid_file="$2"
  local log_file="$3"
  local probe_type="$4"
  local probe_target="$5"
  shift 5

  if [ -f "${pid_file}" ]; then
    local existing_pid
    existing_pid="$(cat "${pid_file}")"
    if kill -0 "${existing_pid}" 2>/dev/null; then
      if [ "${probe_type}" = "http" ] && wait_for_http "${probe_target}" 2 1; then
        echo "ℹ️ ${name} 已在运行，PID: ${existing_pid}"
        echo "   日志: ${log_file}"
        return 0
      fi
      if [ "${probe_type}" = "port" ] && wait_for_port "${probe_target}" 2 1; then
        echo "ℹ️ ${name} 已在运行，PID: ${existing_pid}"
        echo "   日志: ${log_file}"
        return 0
      fi
    fi
    rm -f "${pid_file}"
  fi

  nohup "$@" > "${log_file}" 2>&1 < /dev/null &
  local pid=$!
  echo "${pid}" > "${pid_file}"

  if [ "${probe_type}" = "http" ] && wait_for_http "${probe_target}"; then
    echo "✅ ${name} 已启动，PID: ${pid}"
    echo "   日志: ${log_file}"
    return 0
  fi

  if [ "${probe_type}" = "port" ] && wait_for_port "${probe_target}"; then
    echo "✅ ${name} 已启动，PID: ${pid}"
    echo "   日志: ${log_file}"
    return 0
  fi

  echo "❌ ${name} 启动失败。最近日志如下："
  tail -n 80 "${log_file}" 2>/dev/null || true
  exit 1
}

# 3. 启动 Gateway 与 Channel Server
echo "🚀 正在启动 DavisZeroClaw 官方运行时进程 (后台运行)..."
echo "ℹ️ 采用 zeroclaw gateway + zeroclaw channel start，以确保 Webhook Channel 真正监听。"

start_process "Gateway" "${GATEWAY_PID_FILE}" "${GATEWAY_LOG}" "http" "http://127.0.0.1:3000/health" \
  zeroclaw gateway start --host 0.0.0.0

start_process "Channel Server" "${CHANNEL_PID_FILE}" "${CHANNEL_LOG}" "port" "3001" \
  zeroclaw channel start

echo "🌐 Gateway 健康检查: http://<mac-ip>:3000/health"
echo "🔗 Shortcut Webhook Channel: http://<mac-ip>:3001/shortcut"
echo "🛑 停止服务: ./scripts/stop_davis.sh"
