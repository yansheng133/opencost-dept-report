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

k apply --dry-run=$DRY -f "$HERE/k8s/cost-report.yaml" || exit 1
# ConfigMap 由程式碼產生，不手寫：改了 app.py／index.html 重跑就會更新
k create configmap cost-report-code -n "$NS" \
    --from-file="$HERE/app/app.py" --from-file="$HERE/app/index.html" \
    --dry-run=client -o yaml | k apply --dry-run=$DRY -f - || exit 1

if [ $APPLY = 1 ]; then
  # 內容變了要讓 Pod 重新掛載：用註記觸發滾動更新
  sum=$(cat "$HERE/app/app.py" "$HERE/app/index.html" | shasum -a 256 | cut -c1-12)
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
