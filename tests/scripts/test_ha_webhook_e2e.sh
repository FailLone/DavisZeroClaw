#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "${SCRIPT_DIR}/common.sh"
SESSIONS_DIR="${RUNTIME_DIR}/workspace/sessions"
DAEMON_LOG="${RUNTIME_DIR}/daemon.log"

require_http_ok() {
  local url="$1"
  local name="$2"
  curl -fsS "${url}" >/dev/null 2>&1 || fail "${name} 未就绪：${url}"
}

read_webhook_secret() {
  local runtime_config="${RUNTIME_DIR}/config.toml"
  if [ ! -f "${runtime_config}" ]; then
    return 0
  fi

  awk '
    /^\[channels_config\.webhook\]/ { in_section=1; next }
    /^\[/ { in_section=0 }
    in_section && $1 == "secret" {
      line = $0
      sub(/^[^=]*=[[:space:]]*"/, "", line)
      sub(/"[[:space:]]*$/, "", line)
      print line
      exit
    }
  ' "${runtime_config}"
}

post_shortcut() {
  local thread_id="$1"
  local content="$2"
  local payload
  local webhook_secret
  payload="$(printf '{"sender":"ios-shortcuts","content":"%s","thread_id":"%s"}' "${content}" "${thread_id}")"
  webhook_secret="$(read_webhook_secret)"
  if [ -n "${webhook_secret}" ]; then
    curl -fsS -X POST "http://127.0.0.1:3001/shortcut" \
      -H "Content-Type: application/json" \
      -H "X-Webhook-Secret: ${webhook_secret}" \
      -d "${payload}" >/dev/null
    return 0
  fi

  curl -fsS -X POST "http://127.0.0.1:3001/shortcut" \
    -H "Content-Type: application/json" \
    -d "${payload}" >/dev/null
}

wait_for_session_file() {
  local session_file="$1"
  local attempts="${2:-15}"
  local delay="${3:-1}"
  local i
  for ((i = 0; i < attempts; i++)); do
    if [ -f "${session_file}" ]; then
      return 0
    fi
    sleep "${delay}"
  done
  return 1
}

wait_for_session_assistant() {
  local session_file="$1"
  local attempts="${2:-45}"
  local delay="${3:-1}"
  local i
  for ((i = 0; i < attempts; i++)); do
    if [ -f "${session_file}" ] && grep -Fq '"role":"assistant"' "${session_file}"; then
      return 0
    fi
    sleep "${delay}"
  done
  return 1
}

assert_session_contains() {
  local session_file="$1"
  local needle="$2"
  local label="$3"
  grep -Fq "${needle}" "${session_file}" || {
    echo "---- ${label} session ----" >&2
    cat "${session_file}" >&2
    echo "-------------------------" >&2
    fail "${label} 未包含预期内容：${needle}"
  }
}

assert_session_not_contains() {
  local session_file="$1"
  local needle="$2"
  local label="$3"
  if grep -Fq "${needle}" "${session_file}"; then
    echo "---- ${label} session ----" >&2
    cat "${session_file}" >&2
    echo "-------------------------" >&2
    fail "${label} 出现了不应出现的内容：${needle}"
  fi
}

print_recent_debug() {
  local session_file="$1"
  echo "---- recent daemon log ----" >&2
  tail -n 40 "${DAEMON_LOG}" >&2 || true
  if [ -f "${session_file}" ]; then
    echo "---- session file ----" >&2
    cat "${session_file}" >&2 || true
  fi
  echo "--------------------------" >&2
}

info "检查 Davis 与 ZeroClaw 健康状态..."
require_http_ok "http://127.0.0.1:3010/health" "Davis HA Proxy"
require_http_ok "http://127.0.0.1:3000/health" "ZeroClaw Gateway"
pass "基础健康状态正常"

qa_thread="e2e-qa-$(date +%s)"
qa_session="${SESSIONS_DIR}/webhook_${qa_thread}_${qa_thread}_ios-shortcuts.jsonl"
info "验证 webhook 问答链路..."
post_shortcut "${qa_thread}" "你好，请用一句话回复当前模型路由是否可用。"
wait_for_session_file "${qa_session}" 10 1 || {
  print_recent_debug "${qa_session}"
  fail "问答链路没有生成 session 文件"
}
wait_for_session_assistant "${qa_session}" 25 1 || {
  print_recent_debug "${qa_session}"
  fail "问答链路在超时时间内没有得到 assistant 回复"
}
assert_session_contains "${qa_session}" "当前模型路由" "问答链路"
pass "webhook 问答链路正常"

control_thread="e2e-control-$(date +%s)"
control_session="${SESSIONS_DIR}/webhook_${control_thread}_${control_thread}_ios-shortcuts.jsonl"
info "验证 webhook 控制链路..."
post_shortcut "${control_thread}" "请把书房灯带打开一下"
wait_for_session_file "${control_session}" 10 1 || {
  print_recent_debug "${control_session}"
  fail "控制链路没有生成 session 文件"
}
wait_for_session_assistant "${control_session}" 45 1 || {
  print_recent_debug "${control_session}"
  fail "控制链路在超时时间内没有得到 assistant 回复"
}
assert_session_contains "${control_session}" "书房灯带" "控制链路"
assert_session_contains "${control_session}" "打开" "控制链路"
assert_session_not_contains "${control_session}" "抱歉" "控制链路"
assert_session_not_contains "${control_session}" "无法" "控制链路"
pass "webhook 控制链路正常"

audit_thread="e2e-audit-$(date +%s)"
audit_session="${SESSIONS_DIR}/webhook_${audit_thread}_${audit_thread}_ios-shortcuts.jsonl"
info "验证 webhook 历史审计链路..."
post_shortcut "${audit_thread}" "请调查最近一小时内书房灯带有没有被打开过，只允许读取，不要执行任何写操作。"
wait_for_session_file "${audit_session}" 10 1 || {
  print_recent_debug "${audit_session}"
  fail "审计链路没有生成 session 文件"
}
wait_for_session_assistant "${audit_session}" 45 1 || {
  print_recent_debug "${audit_session}"
  fail "审计链路在超时时间内没有得到 assistant 回复"
}
assert_session_contains "${audit_session}" "最近一小时内" "审计链路"
assert_session_not_contains "${audit_session}" "无法" "审计链路"
assert_session_not_contains "${audit_session}" "配置存在问题" "审计链路"
pass "webhook 历史审计链路正常"

pass "HA webhook 端到端回归测试通过"
