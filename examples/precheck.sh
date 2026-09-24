#!/usr/bin/env bash
# OpenCost demo 開會前檢查：逐項印 PASS／FAIL，任一項 FAIL 就以非 0 結束。
# 用法：KCFG=<下游 kubeconfig> bash precheck.sh          （資料長度門檻預設 24 小時）
#       MIN_HOURS=48 KCFG=… bash precheck.sh
#       MIN_HOURS=0  KCFG=… bash precheck.sh             （只在「故意弄壞要會 FAIL」測試時用 0，否則新環境永遠 FAIL）
# 可覆寫：DEPTS="mfg rd it"  COST_LABEL=cost-center  NAMESPACES="…"  UI_PORT=9090  API_PORT=9003
# 前提：本機已把 OpenCost UI／API 轉送到 127.0.0.1:UI_PORT／API_PORT（見 SKILL.md「本機轉送」）。
# 不讀取、不印出 kubeconfig 內容，也不含任何憑證。
case "${1:-}" in -h|--help) sed -n '2,8p' "$0"; exit 0 ;; esac
set -uo pipefail

KCFG="${KCFG:?請 export KCFG=<下游叢集 kubeconfig>}"
MIN_HOURS="${MIN_HOURS:-24}"
COST_LABEL="${COST_LABEL:-cost-center}"
UI_PORT="${UI_PORT:-9090}"; API_PORT="${API_PORT:-9003}"
read -r -a DEPTS <<<"${DEPTS:-mfg rd it}"
read -r -a NAMESPACES <<<"${NAMESPACES:-opencost prometheus-system local-path-storage dept-mfg dept-rd dept-it}"
PROM=/api/v1/namespaces/prometheus-system/services/prometheus-server:80/proxy/api/v1

FAILS=0
pass() { printf '  \033[32mPASS\033[0m  %s\n' "$1"; }
fail() { printf '  \033[31mFAIL\033[0m  %s\n' "$1"; FAILS=$((FAILS+1)); }
k()    { kubectl --kubeconfig "$KCFG" --request-timeout=15s "$@"; }
promq() { k get --raw "$PROM/query?query=$(jq -rn --arg q "$1" '$q|@uri')" 2>/dev/null; }

echo "OpenCost demo precheck  $(date '+%Y-%m-%d %H:%M:%S')"

# 1. kubeconfig 可用，節點 Ready
echo "[1] 叢集連線"
if [ ! -r "$KCFG" ]; then
  fail "找不到 kubeconfig：$KCFG"
else
  ready="$(k get nodes -o jsonpath='{range .items[*]}{.metadata.name}={.status.conditions[?(@.type=="Ready")].status}{" "}{end}' 2>/dev/null)"
  if [ -n "$ready" ] && ! grep -q '=False\|=Unknown' <<<"$ready"; then pass "節點 Ready：$ready"; else fail "節點未 Ready 或連不上 API：${ready:-（無回應）}"; fi
fi

# 2. 每個 namespace 至少有一個 Pod，且所有 Pod Running、所有容器 ready
echo "[2] Pod 狀態"
for ns in "${NAMESPACES[@]}"; do
  st="$(k get pods -n "$ns" -o json 2>/dev/null | jq -r '
        [.items[] | select(.status.phase!="Succeeded")] as $p
        | if ($p|length)==0 then "EMPTY"
          else ($p | map(select(.status.phase!="Running" or ([.status.containerStatuses[]?.ready]|all|not)) | .metadata.name) | join(","))
          end')"
  if [ -z "$st" ] && k get ns "$ns" >/dev/null 2>&1; then pass "$ns 全部 Running 且 ready"
  elif [ "$st" = "EMPTY" ]; then fail "$ns 沒有任何 Pod"
  else fail "$ns 有 Pod 未就緒：${st:-（無法查詢）}"; fi
done

# 3. UI 與 API 轉送：要回得到 OpenCost 的 JSON，不只是 port 有開
echo "[3] 本機轉送"
for p in "$UI_PORT" "$API_PORT"; do
  bind="$(lsof -nP -iTCP:$p -sTCP:LISTEN 2>/dev/null | awk 'NR>1{print $9}' | sort -u | tr '\n' ' ')"
  if [ -n "$bind" ] && ! grep -qv '^127\.0\.0\.1:' <<<"$(tr ' ' '\n' <<<"$bind" | sed '/^$/d')"; then pass "$p 只綁 127.0.0.1（${bind}）"; else fail "$p 沒有監聽或不是只綁 127.0.0.1：${bind:-（無）}"; fi
done
path_ui="http://127.0.0.1:${UI_PORT}/model/allocation/compute?window=10m&aggregate=namespace"
path_api="http://127.0.0.1:${API_PORT}/allocation/compute?window=10m&aggregate=namespace"
for u in "$path_ui" "$path_api"; do
  code="$(curl -s -m 20 "$u" 2>/dev/null | jq -r '.code // empty' 2>/dev/null)"
  if [ "$code" = "200" ]; then pass "OpenCost 回應 JSON code=200：${u%%\?*}"; else fail "沒有拿到 OpenCost 的 JSON：${u%%\?*}"; fi
done

# 4. Prometheus 有在抓 OpenCost
echo "[4] Prometheus"
h="$(promq 'up{job="opencost"}' | jq -r '[.data.result[].value[1]] | if length==0 then "none" else join(",") end' 2>/dev/null)"
if [ "$h" = "1" ]; then pass "opencost scrape target UP"; else fail "opencost scrape target 不是 UP（up=${h:-查詢失敗}）"; fi

# 5. allocation API 依 COST_LABEL 分出每個部門，且金額 > 0、資料是新的（15 分鐘內）
echo "[5] 部門成本"
J="$(curl -s -m 30 "http://127.0.0.1:${API_PORT}/allocation/compute?window=1h&aggregate=label:${COST_LABEL}&accumulate=true" 2>/dev/null)"
now=$(date -u +%s)
for d in "${DEPTS[@]}"; do
  line="$(jq -r --arg d "$d" '.data[0][$d] // empty | "\(.totalCost) \(.end)"' <<<"$J" 2>/dev/null)"
  cost="${line%% *}"; end="${line##* }"
  if [ -z "$line" ]; then fail "${COST_LABEL}=$d 沒有出現在 API 結果"; continue; fi
  endts=$(date -j -u -f '%Y-%m-%dT%H:%M:%SZ' "$end" +%s 2>/dev/null || echo 0)
  age=$(( (now - endts) / 60 ))
  if awk "BEGIN{exit !($cost>0)}" && [ "$age" -le 15 ]; then pass "${COST_LABEL}=$d totalCost=${cost}（最新資料 ${age} 分鐘前）"
  else fail "${COST_LABEL}=$d totalCost=${cost}，最新資料 ${age} 分鐘前（需 >0 且 ≤15 分鐘）"; fi
done

# 6. 資料長度 ≥ MIN_HOURS，並檢查最近 24 小時（或全部資料期間）的連續性
echo "[6] 資料長度"
hrs="$(promq 'time()-prometheus_tsdb_lowest_timestamp_seconds' | jq -r '.data.result[0].value[1] // empty' 2>/dev/null)"
if [ -z "$hrs" ]; then fail "查不到 Prometheus 資料起點"
else
  hrs=$(awk "BEGIN{printf \"%.1f\", $hrs/3600}")
  if awk "BEGIN{exit !($hrs>=$MIN_HOURS)}"; then pass "已累積 ${hrs} 小時（門檻 ${MIN_HOURS}）"; else fail "只累積 ${hrs} 小時，未達 ${MIN_HOURS} 小時"; fi
  # 連續性：opencost target 實際樣本數 ÷ 依抓取間隔應有的樣本數
  # 分母從 opencost target 自己的第一筆樣本算起（它比 Prometheus 晚裝），最多看最近 24 小時
  oc_age="$(promq 'time() - min_over_time(timestamp(up{job="opencost"})[24h:1m])' | jq -r '.data.result[0].value[1] // 0' 2>/dev/null)"
  span=$(awk "BEGIN{s=${oc_age:-0}; if(s>86400)s=86400; printf \"%d\", s}")
  if [ "$span" -lt 600 ]; then fail "opencost target 的樣本不足 10 分鐘，無法判斷連續性"
  else
    cov="$(promq "count_over_time(up{job=\"opencost\"}[${span}s]) * scalar(avg(prometheus_target_interval_length_seconds{quantile=\"0.5\"})) / ${span}" | jq -r '.data.result[0].value[1] // empty' 2>/dev/null)"
    pct=$(awk -v c="${cov:-0}" 'BEGIN{printf "%.1f", c*100}')
    if [ -n "$cov" ] && awk "BEGIN{exit !($cov>=0.95)}"; then pass "資料連續性 ${pct}%（最近 $((span/60)) 分鐘，門檻 95%）"
    else fail "資料有斷層：連續性 ${cov:-查詢失敗}（最近 $((span/60)) 分鐘，門檻 0.95）"; fi
  fi
fi

echo
if [ "$FAILS" -eq 0 ]; then echo "結果：全部 PASS"; exit 0; else echo "結果：$FAILS 項 FAIL"; exit 1; fi
