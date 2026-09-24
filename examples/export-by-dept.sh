#!/usr/bin/env bash
# 依成本標籤（預設 cost-center）匯出部門成本 CSV（交給財務的報表範例）
# 用法：bash export-by-dept.sh [--dept <部門>] [window]
#   不帶 --dept：各部門彙總一列（含 __idle__／__unallocated__）
#   帶 --dept：只列該部門，逐一工作負載（controller）一列，含 cpu／ram 效率；
#              依 Pod 的標籤歸屬，所以跑在別的 namespace、但標成該部門的工作負載也會列入
#   window 預設 7d；也可給 24h、2d，或 RFC3339 起訖 "2026-09-22T00:00:00Z,2026-09-23T00:00:00Z"
# 可覆寫：DEPTS="mfg rd it"（--dept 的合法值）  COST_LABEL=cost-center  API_PORT=9003  OUTDIR=./exports
# 資料來源：本機 OpenCost API 轉送 127.0.0.1:API_PORT。不讀 kubeconfig，不含任何憑證。
case "${1:-}" in -h|--help) sed -n '2,9p' "$0"; exit 0 ;; esac
set -euo pipefail

COST_LABEL="${COST_LABEL:-cost-center}"
DEPTS="${DEPTS:-mfg rd it}"
DEPT=""
if [ "${1:-}" = "--dept" ]; then
  DEPT="${2:-}"; shift 2 || true
  [[ " $DEPTS " == *" $DEPT "* ]] && [ -n "$DEPT" ] || { echo "FAIL: --dept 只接受 ${DEPTS}（收到「${DEPT}」）" >&2; exit 1; }
fi
WINDOW="${1:-7d}"
API="http://127.0.0.1:${API_PORT:-9003}/allocation/compute"
OUTDIR="${OUTDIR:-$PWD/exports}"
SAFE_WIN="$(printf '%s' "$WINDOW" | tr ':,' '-_')"
if [ -n "$DEPT" ]; then
  OUT="$OUTDIR/cost-${DEPT}_$(date +%Y%m%d-%H%M)_${SAFE_WIN}.csv"
else
  OUT="$OUTDIR/cost-by-dept_$(date +%Y%m%d-%H%M)_${SAFE_WIN}.csv"
fi

command -v jq >/dev/null || { echo "FAIL: 需要 jq" >&2; exit 1; }
mkdir -p "$OUTDIR"

if [ -n "$DEPT" ]; then
  JSON="$(curl -sf -m 60 -G "$API" \
    --data-urlencode "window=$WINDOW" \
    --data-urlencode "aggregate=controller" \
    --data-urlencode "filter=label[${COST_LABEL}]:\"$DEPT\"" \
    --data-urlencode "accumulate=true")" \
    || { echo "FAIL: 連不到 OpenCost API（${API}），轉送是否在跑？" >&2; exit 1; }
else
  JSON="$(curl -sf -m 60 -G "$API" \
    --data-urlencode "window=$WINDOW" \
    --data-urlencode "aggregate=label:${COST_LABEL}" \
    --data-urlencode "accumulate=true" \
    --data-urlencode "includeIdle=true" \
    --data-urlencode "shareIdle=false")" \
    || { echo "FAIL: 連不到 OpenCost API（${API}），轉送是否在跑？" >&2; exit 1; }
fi

[ "$(jq -r '.code' <<<"$JSON")" = "200" ] || { echo "FAIL: API 回傳非 200：$(jq -c '{code,message}' <<<"$JSON")" >&2; exit 1; }

if [ -n "$DEPT" ]; then
  [ "$(jq '.data[0] | length' <<<"$JSON")" -gt 0 ] || { echo "FAIL: ${COST_LABEL}=${DEPT} 在 window=${WINDOW} 沒有任何工作負載" >&2; exit 1; }
  # 逐一工作負載；金額四捨五入到小數 4 位，效率到小數 3 位
  jq -r --arg d "$DEPT" '
    def r4: (. * 10000 | round) / 10000;
    def r3: (. * 1000 | round) / 1000;
    ["department","namespace","workload","window_start","window_end","cpu_cost","ram_cost","pv_cost","total_cost","cpu_efficiency","ram_efficiency"],
    ( .data[0] | to_entries | sort_by(.value.properties.namespace, .key)[]
      | .value
      | [$d, .properties.namespace, .name, .start, .end, (.cpuCost|r4), (.ramCost|r4), (.pvCost|r4), (.totalCost|r4), (.cpuEfficiency|r3), (.ramEfficiency|r3)] )
    | @csv' <<<"$JSON" > "$OUT"
else
  # 部門列在前，__idle__／__unallocated__ 放最後；金額欄四捨五入到小數 4 位
  jq -r '
    def r4: (. * 10000 | round) / 10000;
    ["department","window_start","window_end","cpu_cost","ram_cost","pv_cost","total_cost"],
    ( .data[0] | to_entries
      | sort_by((.key|startswith("__")), .key)[]
      | .value
      | [.name, .start, .end, (.cpuCost|r4), (.ramCost|r4), (.pvCost|r4), (.totalCost|r4)] )
    | @csv' <<<"$JSON" > "$OUT"
fi

echo "已匯出：${OUT}（window=${WINDOW}${DEPT:+，dept=${DEPT}}）"
