#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "${SCRIPT_DIR}/common.sh"

WRITE_MODE=0

for arg in "$@"; do
  case "${arg}" in
    --write)
      WRITE_MODE=1
      ;;
    -h|--help)
      cat <<'EOF'
用法：
  ./tests/scripts/test_ha_real.sh
  ./tests/scripts/test_ha_real.sh --write

默认模式只做真实 HA 读路径与控制解析验证，不执行写操作。
加上 --write 后，会通过 execute-control 真实执行：
  - 打开书房灯带
  - 关闭书房灯带
EOF
      exit 0
      ;;
    *)
      echo "❌ 不支持的参数: ${arg}" >&2
      exit 1
      ;;
  esac
done

require_running_service() {
  local url="$1"
  local name="$2"
  if ! curl -fsS "${url}" >/dev/null 2>&1; then
    fail "${name} 未就绪：${url}"
  fi
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local label="$3"
  if ! printf '%s' "${haystack}" | grep -Fq "${needle}"; then
    echo "---- ${label} 响应 ----" >&2
    printf '%s\n' "${haystack}" >&2
    echo "----------------------" >&2
    fail "${label} 未包含预期内容：${needle}"
  fi
}

assert_not_contains_any() {
  local haystack="$1"
  local label="$2"
  shift 2
  local needle
  for needle in "$@"; do
    if printf '%s' "${haystack}" | grep -Fq "${needle}"; then
      echo "---- ${label} 响应 ----" >&2
      printf '%s\n' "${haystack}" >&2
      echo "----------------------" >&2
      fail "${label} 出现了不应出现的内容：${needle}"
    fi
  done
}

info "检查 Davis 和 ZeroClaw 是否已启动..."
require_running_service "http://127.0.0.1:3010/health" "Davis Local Proxy"
require_running_service "http://127.0.0.1:3000/health" "ZeroClaw Gateway"

route_status="$(curl -fsS "http://127.0.0.1:3010/model-routing/status")"
assert_contains "${route_status}" "\"route_ready\":true" "model-routing/status"
pass "模型路由已就绪"

info "验证 Home Assistant MCP 能力探测..."
ha_mcp_capabilities="$(curl -fsS "http://127.0.0.1:3010/ha-mcp/capabilities")"
assert_contains "${ha_mcp_capabilities}" "\"supports_live_context\":true" "ha-mcp/capabilities"
assert_contains "${ha_mcp_capabilities}" "\"supports_audit_history\":false" "ha-mcp/capabilities"
pass "HA MCP 能力探测正常，且当前未暴露历史审计工具"

info "验证 Home Assistant MCP 实时上下文只读诊断..."
ha_mcp_live_context="$(curl -fsS "http://127.0.0.1:3010/ha-mcp/live-context")"
assert_contains "${ha_mcp_live_context}" "\"status\":\"ok\"" "ha-mcp/live-context"
assert_contains "${ha_mcp_live_context}" "\"source_tool\":\"GetLiveContext\"" "ha-mcp/live-context"
assert_contains "${ha_mcp_live_context}" "\"preview\"" "ha-mcp/live-context"
pass "HA MCP 实时上下文诊断正常"

info "验证真实 HA 控制解析..."
study_resolve="$(curl -fsS 'http://127.0.0.1:3010/resolve-control-target?query=%E4%B9%A6%E6%88%BF%E7%81%AF%E5%B8%A6&action=turn_on')"
assert_contains "${study_resolve}" "\"status\":\"ok\"" "书房灯带控制解析"
assert_contains "${study_resolve}" "light.shu_fang_deng_dai" "书房灯带控制解析"
pass "书房灯带控制解析正常"

info "验证真实 HA 当前状态查询..."
query_state_payload="$(curl -fsS -X POST 'http://127.0.0.1:3010/execute-control' \
  -H 'Content-Type: application/json' \
  -d '{"query":"书房灯带","action":"query_state"}')"
assert_contains "${query_state_payload}" "\"status\":\"success\"" "书房灯带状态查询"
assert_not_contains_any "${query_state_payload}" "书房灯带状态查询" "ha_unreachable" "ha_auth_failed" "missing_credentials"
pass "书房灯带状态查询正常"

info "验证真实 HA 只读审计链路..."
audit_payload="$(curl -fsS 'http://127.0.0.1:3010/audit?entity_id=light.shu_fang_deng_dai&hours=1')"
assert_contains "${audit_payload}" "\"result_type\"" "书房灯带只读审计"
assert_not_contains_any "${audit_payload}" "书房灯带只读审计" \
  "ha_unreachable" "ha_auth_failed" "missing_credentials" "entity_not_found" "bad_request"
pass "书房灯带只读审计正常"

if [ "${WRITE_MODE}" -eq 1 ]; then
  info "执行真实写操作回归：通过 execute-control 打开/关闭书房灯带"

  turn_on_payload="$(curl -fsS -X POST 'http://127.0.0.1:3010/execute-control' \
    -H 'Content-Type: application/json' \
    -d '{"query":"书房灯带","action":"turn_on"}')"
  assert_contains "${turn_on_payload}" "\"status\":\"success\"" "打开书房灯带"
  assert_not_contains_any "${turn_on_payload}" "打开书房灯带" \
    "ha_unreachable" "ha_auth_failed" "missing_credentials" "resolution_ambiguous" "execution_failed"
  pass "打开书房灯带真实写操作正常"

  turn_off_payload="$(curl -fsS -X POST 'http://127.0.0.1:3010/execute-control' \
    -H 'Content-Type: application/json' \
    -d '{"query":"书房灯带","action":"turn_off"}')"
  assert_contains "${turn_off_payload}" "\"status\":\"success\"" "关闭书房灯带"
  assert_not_contains_any "${turn_off_payload}" "关闭书房灯带" \
    "ha_unreachable" "ha_auth_failed" "missing_credentials" "resolution_ambiguous" "execution_failed"
  pass "关闭书房灯带真实写操作正常"
fi

pass "HA 真实回归测试通过"
