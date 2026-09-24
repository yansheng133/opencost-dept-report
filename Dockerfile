# 部門成本分攤報表。只用 Python 標準函式庫，不裝任何套件。
FROM registry.suse.com/bci/python:3.12

LABEL org.opencontainers.image.title="cost-report" \
      org.opencontainers.image.description="依 cost-center 標籤呈現 OpenCost 成本分攤的唯讀報表" \
      org.opencontainers.image.source="https://example.internal/cost-report"

WORKDIR /app
COPY app/app.py app/index.html /app/

# 以非 root 執行；bci 映像檔沒有預設非 root 使用者，這裡指定 UID（不必建帳號，Kubernetes 也會再指定一次）
USER 1000:1000

ENV OPENCOST_URL=http://opencost.opencost.svc.cluster.local:9003 \
    CACHE_TTL=60 \
    WINDOWS=1h,24h,7d \
    DEFAULT_WINDOW=24h \
    LISTEN_PORT=8080 \
    PYTHONUNBUFFERED=1

EXPOSE 8080
# 沒有 shell 的健康檢查：Kubernetes 用 httpGet /healthz、/readyz
CMD ["python3", "/app/app.py"]
