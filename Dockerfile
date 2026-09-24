# 部門成本分攤報表。只用 Python 標準函式庫，不裝任何套件。
# 基礎映像檔有 amd64／arm64（查過 manifest list），所以 buildx 的多架構建置不必換底。
FROM registry.suse.com/bci/python:3.12

ARG VERSION=dev
ARG REVISION=unknown
LABEL org.opencontainers.image.title="cost-report" \
      org.opencontainers.image.description="依 cost-center 標籤呈現 OpenCost 成本分攤的唯讀報表" \
      org.opencontainers.image.source="https://github.com/yansheng133/opencost-dept-report" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}"

WORKDIR /app
COPY app/app.py app/billing.py app/index.html /app/
# 價目表烤一份預設值進來，正式環境用 ConfigMap 蓋掉 /config/ratecard.json。
# 單價是政策不是程式——改價不該需要重新建置映像檔。
COPY app/ratecard.json /config/ratecard.json

# 以非 root 執行。bci 映像檔沒有預設的非 root 使用者，這裡直接指定 UID；
# Kubernetes 會再指定一次（runAsUser），兩邊一致才不會在別的叢集上踩到。
USER 1000:1000

ENV OPENCOST_URL=http://opencost.opencost.svc.cluster.local:9003 \
    CACHE_TTL=60 \
    WINDOWS=1h,24h,7d \
    DEFAULT_WINDOW=24h \
    LISTEN_PORT=8080 \
    RATECARD_PATH=/config/ratecard.json \
    SEAL_DIR=/seals \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

EXPOSE 8080
# 沒有 shell 的健康檢查：Kubernetes 用 httpGet /healthz、/readyz
CMD ["python3", "/app/app.py"]
