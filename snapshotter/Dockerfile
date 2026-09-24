# 宣告量快照器。跟報表服務一樣：只用 Python 標準函式庫，不裝任何套件。
FROM registry.suse.com/bci/python:3.12

ARG VERSION=dev
ARG REVISION=unknown
LABEL org.opencontainers.image.title="cost-snapshotter" \
      org.opencontainers.image.description="定期把 Pod 的資源宣告量與部門標籤存成快照，作為 Prometheus 斷線時的第二份分攤依據" \
      org.opencontainers.image.source="https://github.com/yansheng133/opencost-dept-report" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}"

WORKDIR /app
# fallback.py 是離線分析用的 CLI，它 import snapshot.py。一起裝進來，
# 需要對帳的時候可以直接 kubectl exec 進 Pod 跑，不必先把快照撈出去。
COPY snapshot.py fallback.py /app/

USER 1000:1000

ENV OUT_DIR=/data \
    INTERVAL=300 \
    DEPT_LABEL=cost-center \
    ENV_LABEL=env \
    RETENTION_DAYS=35 \
    LISTEN_PORT=8080 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

EXPOSE 8080
CMD ["python3", "/app/snapshot.py"]
