#!/usr/bin/env bash
# 部署宣告量快照器（不需要 registry：程式碼放 ConfigMap，掛進官方 Python 映像檔）
# 用法：KCFG=<下游 kubeconfig> bash deploy.sh [--apply]
#   不加 --apply 只做 dry-run。改了 snapshot.py 重跑即可，會自動重啟 Pod。
case "${1:-}" in -h|--help) sed -n '2,5p' "$0"; exit 0 ;; esac
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
KCFG="${KCFG:?請 export KCFG=<下游叢集 kubeconfig>}"
NS=cost-report
APPLY=0; [ "${1:-}" = --apply ] && APPLY=1
k() { kubectl --kubeconfig "$KCFG" "$@"; }

echo "模式：$([ $APPLY = 1 ] && echo APPLY || echo DRY-RUN)　叢集：$(k config current-context 2>/dev/null)"
DRY=$([ $APPLY = 1 ] && echo none || echo server)
if [ $APPLY = 0 ] && ! k get ns "$NS" >/dev/null 2>&1; then
  DRY=client
  echo "（namespace ${NS} 還不存在，dry-run 改用用戶端驗證；請先部署 cost-report）"
fi

k apply --dry-run=$DRY -f "$HERE/k8s.yaml" || exit 1
k create configmap cost-snapshotter-code -n "$NS" \
    --from-file="$HERE/snapshot.py" \
    --dry-run=client -o yaml | k apply --dry-run=$DRY -f - || exit 1

if [ $APPLY = 1 ]; then
  sum=$(shasum -a 256 "$HERE/snapshot.py" | cut -c1-12)
  k -n "$NS" patch deploy cost-snapshotter --type merge \
    -p "{\"spec\":{\"template\":{\"metadata\":{\"annotations\":{\"cost-report/code-sha\":\"${sum}\"}}}}}" >/dev/null 2>&1
  k -n "$NS" rollout status deploy/cost-snapshotter --timeout=180s
  echo
  echo "看它有沒有在寫：k -n ${NS} logs deploy/cost-snapshotter --tail=5"
  echo "把快照取出來對帳："
  echo "  pod=\$(k -n ${NS} get pod -l app=cost-snapshotter -o jsonpath='{.items[0].metadata.name}')"
  echo "  k -n ${NS} cp \$pod:/data ./snapshots"
  echo "  python3 fallback.py --snapshots ./snapshots --start <RFC3339> --end <RFC3339>"
else
  echo; echo "（dry-run：沒有寫入叢集。加 --apply 才部署。）"
fi
