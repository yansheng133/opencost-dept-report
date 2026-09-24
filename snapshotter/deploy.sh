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

# PVC 要的 StorageClass：叢集不一定有預設的，而少了它只會讓 PVC 無聲 Pending，
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
      SC="${ALL}"
      echo "StorageClass：叢集沒有預設的，只有 ${SC} 一個，就用它"
    else
      echo "錯誤：叢集沒有預設 StorageClass，請指定 STORAGE_CLASS=<名稱>" >&2
      echo "      可用的有：${ALL:-（一個都沒有，要先裝 provisioner）}" >&2
      exit 1
    fi
  fi
else
  echo "StorageClass：使用指定的 ${SC}"
fi

MANIFEST="$HERE/k8s.yaml"
if [ -n "${SC}" ]; then
  MANIFEST=$(mktemp -t snapshotter-k8s)
  trap 'rm -f "${MANIFEST}"' EXIT
  SC="${SC}" python3 - "$HERE/k8s.yaml" > "${MANIFEST}" <<'PYEOF'
import os, sys
src = open(sys.argv[1]).read()
anchor = "  resources: {requests: {storage: 1Gi}}"
assert anchor in src, "k8s.yaml 的 PVC 區塊長得跟預期不一樣，不要盲目改寫"
print(src.replace(anchor, anchor + f"\n  storageClassName: {os.environ['SC']}", 1), end="")
PYEOF
fi

k apply --dry-run=$DRY -f "${MANIFEST}" || exit 1
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
