#!/usr/bin/env bash
# 部署部門成本分攤報表到叢集（A 版：用 ConfigMap 掛程式碼，不需要 registry）
# 用法：KCFG=<下游 kubeconfig> bash deploy.sh [--apply]
#   不加 --apply 只做 dry-run。更新程式碼後重跑即可，會自動重啟 Pod。
case "${1:-}" in -h|--help) sed -n '2,5p' "$0"; exit 0 ;; esac
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
KCFG="${KCFG:?請 export KCFG=<下游叢集 kubeconfig>}"
NS=cost-report
APPLY=0; [ "${1:-}" = --apply ] && APPLY=1
k() { kubectl --kubeconfig "$KCFG" "$@"; }

echo "模式：$([ $APPLY = 1 ] && echo APPLY || echo DRY-RUN)　叢集：$(k config current-context 2>/dev/null)"
DRY=$([ $APPLY = 1 ] && echo none || echo server)
# namespace 還不存在時，伺服器端 dry-run 會說「找不到 namespace」，退回用戶端驗證（仍會檢查 schema）
if [ $APPLY = 0 ] && ! k get ns "$NS" >/dev/null 2>&1; then
  DRY=client
  echo "（namespace $NS 還不存在，dry-run 改用用戶端驗證）"
fi

# 封存用的 PVC 需要 StorageClass。叢集不一定有預設的，而少了它只會讓 PVC 無聲 Pending，
# 然後 rollout 等到逾時才吐一個看不出原因的訊息。寧可現在就講清楚。
SC="${STORAGE_CLASS:-}"
if [ -z "${SC}" ]; then
  DEF=$(k get sc -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.metadata.annotations.storageclass\.kubernetes\.io/default-class}{"\n"}{end}' 2>/dev/null | awk -F'\t' '$2=="true"{print $1; exit}')
  if [ -n "${DEF}" ]; then
    echo "StorageClass：使用叢集預設的 ${DEF}"
  else
    ALL=$(k get sc -o jsonpath='{.items[*].metadata.name}' 2>/dev/null)
    N=$(printf '%s' "${ALL}" | wc -w | tr -d ' ')
    if [ "${N}" = 1 ]; then
      SC="${ALL}"; echo "StorageClass：叢集沒有預設的，只有 ${SC} 一個，就用它"
    else
      echo "錯誤：叢集沒有預設 StorageClass，請指定 STORAGE_CLASS=<名稱>" >&2
      echo "      可用的有：${ALL:-（一個都沒有，要先裝 provisioner）}" >&2
      exit 1
    fi
  fi
else
  echo "StorageClass：使用指定的 ${SC}"
fi

MANIFEST="$HERE/k8s/cost-report.yaml"
if [ -n "${SC}" ]; then
  MANIFEST=$(mktemp -t cost-report-k8s)
  trap 'rm -f "${MANIFEST}"' EXIT
  SC="${SC}" python3 - "$HERE/k8s/cost-report.yaml" > "${MANIFEST}" <<'PYEOF'
import os, sys
src = open(sys.argv[1]).read()
anchor = "  resources: {requests: {storage: 1Gi}}"
assert anchor in src, "PVC 區塊長得跟預期不一樣，不要盲目改寫"
print(src.replace(anchor, anchor + f"\n  storageClassName: {os.environ['SC']}", 1), end="")
PYEOF
fi

k apply --dry-run=$DRY -f "${MANIFEST}" || exit 1
# ConfigMap 由程式碼產生，不手寫：改了 app.py／index.html 重跑就會更新
k create configmap cost-report-code -n "$NS" \
    --from-file="$HERE/app/app.py" --from-file="$HERE/app/billing.py" --from-file="$HERE/app/index.html" \
    --dry-run=client -o yaml | k apply --dry-run=$DRY -f - || exit 1
# 價目表獨立一個 ConfigMap：單價是政策不是程式，改價的人跟改程式的人通常不是同一個
k create configmap cost-report-ratecard -n "$NS" \
    --from-file="$HERE/app/ratecard.json" \
    --dry-run=client -o yaml | k apply --dry-run=$DRY -f - || exit 1

if [ $APPLY = 1 ]; then
  # 內容變了要讓 Pod 重新掛載：用註記觸發滾動更新
  sum=$(cat "$HERE/app/app.py" "$HERE/app/billing.py" "$HERE/app/index.html" "$HERE/app/ratecard.json" | shasum -a 256 | cut -c1-12)
  k -n "$NS" patch deploy cost-report --type merge \
    -p "{\"spec\":{\"template\":{\"metadata\":{\"annotations\":{\"cost-report/code-sha\":\"$sum\"}}}}}" >/dev/null 2>&1
  k -n "$NS" rollout status deploy/cost-report --timeout=120s
  echo
  echo "存取方式："
  echo "  1. 本機轉送：kubectl --kubeconfig \$KCFG -n $NS port-forward svc/cost-report 8088:80 --address 127.0.0.1"
  echo "  2. Rancher 代理（沿用 Rancher 的登入與權限）："
  echo "     <Rancher>/k8s/clusters/<叢集 ID>/api/v1/namespaces/$NS/services/http:cost-report:80/proxy/"
  echo "  3. 對外開 Ingress 前請先加認證，見 k8s/ingress-example.yaml"
else
  echo; echo "（dry-run：沒有寫入叢集。加 --apply 才部署。）"
fi
