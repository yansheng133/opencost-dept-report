#!/usr/bin/env bash
# 建置並推送兩個映像檔（linux/amd64 ＋ linux/arm64）
#
# 用法：
#   bash build-images.sh                 只建置不推送（本機驗證用）
#   bash build-images.sh --push          建置並推送到 registry
#   REGISTRY=<帳號> VERSION=<版本> bash build-images.sh --push
#
# 需要 docker buildx 與一個可用的 container runtime（Rancher Desktop 或 Docker Desktop）。
# 多架構建置必須用 buildx 的 container driver——預設的 docker driver 一次只吐得出一個架構，
# 而且失敗訊息不會明講，只會安靜地產出單一架構的映像檔。
set -uo pipefail
case "${1:-}" in -h|--help) sed -n '2,12p' "$0"; exit 0 ;; esac

REGISTRY="${REGISTRY:-yansheng133}"
VERSION="${VERSION:-0.2.0}"
PLATFORMS="${PLATFORMS:-linux/amd64,linux/arm64}"
BUILDER="${BUILDER:-cost-report-builder}"
HERE=$(cd "$(dirname "$0")" && pwd)
PUSH=0; [ "${1:-}" = --push ] && PUSH=1
REV=$(git -C "${HERE}" rev-parse --short HEAD 2>/dev/null || echo unknown)

echo "映像檔　${REGISTRY}/cost-report:${VERSION}、${REGISTRY}/cost-snapshotter:${VERSION}"
echo "架構　　${PLATFORMS}"
echo "模式　　$([ ${PUSH} = 1 ] && echo '建置並推送' || echo '只建置，不推送')"
echo

# Rancher Desktop 在沒有 admin access 的情況下只會建立 ~/.rd/docker.sock，
# 不會去動 /var/run/docker.sock。沒有這一段，腳本會在明明有 runtime 的情況下說連不上。
if [ -z "${DOCKER_HOST:-}" ] && [ ! -S /var/run/docker.sock ] && [ -S "$HOME/.rd/docker.sock" ]; then
  export DOCKER_HOST="unix://$HOME/.rd/docker.sock"
  echo "使用 Rancher Desktop 的 socket：${DOCKER_HOST}"
fi

if ! docker buildx version >/dev/null 2>&1; then
  echo "錯誤：找不到 docker buildx" >&2; exit 1
fi
if ! docker info >/dev/null 2>&1; then
  echo "錯誤：連不上 container runtime。請先啟動 Rancher Desktop 或 Docker Desktop。" >&2
  exit 1
fi

# container driver 才做得出多架構的 manifest list
if ! docker buildx inspect "${BUILDER}" >/dev/null 2>&1; then
  echo "建立 buildx builder：${BUILDER}"
  docker buildx create --name "${BUILDER}" --driver docker-container --use >/dev/null || exit 1
fi
docker buildx use "${BUILDER}" || exit 1
docker buildx inspect --bootstrap >/dev/null || exit 1

if [ ${PUSH} = 1 ]; then
  OUT=(--push)
else
  # 不推送時不能用 --load：docker 的映像檔儲存放不下多架構的 manifest list。
  # 這裡只做建置驗證，產物丟掉。
  OUT=(--output=type=cacheonly)
fi

build() {
  local name=$1 ctx=$2 dockerfile=$3
  echo "── ${name} ─────────────────────────────"
  docker buildx build \
    --platform "${PLATFORMS}" \
    --build-arg "VERSION=${VERSION}" \
    --build-arg "REVISION=${REV}" \
    -t "${REGISTRY}/${name}:${VERSION}" \
    -t "${REGISTRY}/${name}:latest" \
    -f "${dockerfile}" \
    "${OUT[@]}" \
    "${ctx}" || return 1
}

build cost-report      "${HERE}"              "${HERE}/Dockerfile"              || exit 1
build cost-snapshotter "${HERE}/snapshotter"  "${HERE}/snapshotter/Dockerfile"  || exit 1

echo
if [ ${PUSH} = 1 ]; then
  echo "推送完成。驗證實際推上去的架構："
  for n in cost-report cost-snapshotter; do
    echo "  ${REGISTRY}/${n}:${VERSION}"
    docker buildx imagetools inspect "${REGISTRY}/${n}:${VERSION}" 2>/dev/null \
      | grep -E "Platform|Name:" | sed 's/^/    /'
  done
  echo
  echo "部署：IMAGE_TAG=${VERSION} KCFG=<kubeconfig> bash deploy.sh --apply"
else
  echo "（只建置，沒有推送。加 --push 才會上傳。）"
fi
