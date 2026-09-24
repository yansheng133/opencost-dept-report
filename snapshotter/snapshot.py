#!/usr/bin/env python3
"""宣告量快照器：定期把每個 Pod 的資源宣告量與部門標籤存下來。

為什麼需要這個：Prometheus 掛掉的時候，成本還是在發生——節點照跑、雲端照計費、
機器照折舊——但你失去了「這些錢該分給誰」的依據。這支程式只跟 API server 講話，
完全不碰 Prometheus，所以兩者不會同時消失。真的斷線時，這些快照就是第二份分攤依據。

它刻意做得很笨：沒有資料庫、沒有相依套件、一個迴圈把 JSON 寫到磁碟。
備援系統比它保護的系統更複雜的話，就失去意義了。

環境變數：
  OUT_DIR=/data          快照存放目錄
  INTERVAL=300           取樣間隔（秒）
  DEPT_LABEL=cost-center 部門標籤的鍵
  ENV_LABEL=env          第二維度標籤的鍵
  RETENTION_DAYS=35      保留天數（一個帳期加緩衝）
"""
import json
import os
import ssl
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timedelta, timezone

SA = "/var/run/secrets/kubernetes.io/serviceaccount"
OUT_DIR = os.environ.get("OUT_DIR", "/data")
INTERVAL = int(os.environ.get("INTERVAL", "300"))
DEPT_LABEL = os.environ.get("DEPT_LABEL", "cost-center")
ENV_LABEL = os.environ.get("ENV_LABEL", "env")
RETENTION_DAYS = int(os.environ.get("RETENTION_DAYS", "35"))
SCHEMA = 1
KUBECTL = ""          # 空字串＝叢集內模式；否則是 kubeconfig 路徑（或 "1" 用預設 context）


def log(msg):
    print(f"{datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')} {msg}", flush=True)


def api(path):
    """取資料。兩種來源：叢集內用 ServiceAccount，叢集外用本機的 kubectl。

    kubectl 模式是給「部署之前先看看它會抓到什麼」用的，欄位與叢集內完全一樣，
    所以拿它收的快照跟正式的可以混著算。
    """
    if KUBECTL:
        if path.startswith("/api/v1/namespaces") and "pods" not in path:
            args = ["get", "namespaces", "-o", "json"]
        else:
            args = ["get", "pods", "--all-namespaces",
                    "--field-selector=status.phase=Running", "-o", "json"]
        cmd = ["kubectl"] + (["--kubeconfig", KUBECTL] if KUBECTL != "1" else []) + args
        out = subprocess.run(cmd, capture_output=True, check=True).stdout
        return json.loads(out)
    return _api_in_cluster(path)


def _api_in_cluster(path):
    """跟 API server 要資料。

    每次都重新讀 token，不能在啟動時讀一次就好：現在的 ServiceAccount token 是
    有時效的投射式 token，大約一小時就會輪替。快取起來的話，這支程式會在跑了
    一小時之後開始收到 401——而且是在「Prometheus 掛掉、最需要它」的時候才發現。
    """
    with open(f"{SA}/token") as f:
        token = f.read().strip()
    host = os.environ["KUBERNETES_SERVICE_HOST"]
    port = os.environ.get("KUBERNETES_SERVICE_PORT_HTTPS", "443")
    req = urllib.request.Request(
        f"https://{host}:{port}{path}",
        headers={"Authorization": f"Bearer {token}", "Accept": "application/json"},
    )
    ctx = ssl.create_default_context(cafile=f"{SA}/ca.crt")
    with urllib.request.urlopen(req, timeout=30, context=ctx) as r:
        return json.load(r)


def cpu_cores(v):
    """把 CPU 數量字串換算成核心數。"""
    if not v:
        return 0.0
    v = str(v)
    for suffix, div in (("m", 1e3), ("u", 1e6), ("n", 1e9)):
        if v.endswith(suffix):
            return float(v[:-1]) / div
    return float(v)


def mem_bytes(v):
    """把記憶體數量字串換算成位元組。Ki/Mi/Gi 是 2 的冪，K/M/G 是 10 的冪。"""
    if not v:
        return 0.0
    v = str(v)
    units = [("Ki", 2**10), ("Mi", 2**20), ("Gi", 2**30), ("Ti", 2**40),
             ("k", 1e3), ("K", 1e3), ("M", 1e6), ("G", 1e9), ("T", 1e12)]
    for suffix, mult in sorted(units, key=lambda x: -len(x[0])):
        if v.endswith(suffix):
            return float(v[: -len(suffix)]) * mult
    return float(v)


def effective_requests(spec):
    """Pod 的實際宣告量。

    不是「所有容器加起來」就好：排程器取「一般容器總和」與「單一 init container 最大值」
    兩者的較大者，因為 init container 是跑完才換下一個。原生 sidecar（restartPolicy=Always
    的 init container）會跟一般容器並存，所以算進總和。

    只差幾個百分點，但這是「宣告量」這個基準唯一能出錯的地方，錯了會一路錯到帳單上。
    """
    regular = spec.get("containers", []) or []
    inits = spec.get("initContainers", []) or []
    sidecars = [c for c in inits if c.get("restartPolicy") == "Always"]

    def req(c, key, conv):
        return conv((c.get("resources", {}).get("requests", {}) or {}).get(key))

    out = {}
    for key, conv in (("cpu", cpu_cores), ("memory", mem_bytes)):
        # 穩定狀態：一般容器 ＋ 全部 sidecar
        running = sum(req(c, key, conv) for c in regular + sidecars)
        # 啟動期間的峰值：某顆 init container ＋ 排在它前面、已經啟動的 sidecar。
        # 「前面」很重要——宣告在 init 後面的 sidecar 那時還沒起來，不該算進去。
        init_peak, started = 0.0, 0.0
        for c in inits:
            if c.get("restartPolicy") == "Always":
                started += req(c, key, conv)
            else:
                init_peak = max(init_peak, started + req(c, key, conv))
        out[key] = max(running, init_peak)
    return out["cpu"], out["memory"]


def owner_of(pod):
    refs = pod.get("metadata", {}).get("ownerReferences") or []
    if not refs:
        return ""
    ref = refs[0]
    kind, name = ref.get("kind", ""), ref.get("name", "")
    if kind == "ReplicaSet" and "-" in name:
        name = name.rsplit("-", 1)[0]   # ReplicaSet 名稱結尾是 pod-template hash
        kind = "Deployment"
    return f"{kind}/{name}"


def collect():
    """取一次快照：所有 Running 的 Pod，配上它該記到哪個部門。"""
    ns_labels = {}
    for ns in api("/api/v1/namespaces").get("items", []):
        ns_labels[ns["metadata"]["name"]] = ns["metadata"].get("labels") or {}

    pods = api("/api/v1/pods?fieldSelector=status.phase=Running").get("items", [])
    rows = []
    for p in pods:
        meta, spec = p["metadata"], p["spec"]
        ns = meta["namespace"]
        pod_labels = meta.get("labels") or {}
        nsl = ns_labels.get(ns, {})
        # Pod 標籤優先於 namespace 標籤——跟 OpenCost 的行為一致，跨部門計費才算得對
        dept = pod_labels.get(DEPT_LABEL) or nsl.get(DEPT_LABEL) or ""
        envv = pod_labels.get(ENV_LABEL) or nsl.get(ENV_LABEL) or ""
        cpu, mem = effective_requests(spec)
        rows.append({
            "ns": ns, "pod": meta["name"], "dept": dept, "env": envv,
            "cpu": round(cpu, 6), "mem": int(mem),
            "node": spec.get("nodeName", ""), "owner": owner_of(p),
        })
    return rows


def write_snapshot(rows):
    now = datetime.now(timezone.utc)
    day = now.strftime("%Y-%m-%d")
    os.makedirs(f"{OUT_DIR}/{day}", exist_ok=True)
    path = f"{OUT_DIR}/{day}/{now.strftime('%H%M%S')}.json"
    doc = {
        "schema": SCHEMA,
        "ts": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "interval_s": INTERVAL,
        "dept_label": DEPT_LABEL,
        "env_label": ENV_LABEL,
        "pods": rows,
    }
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(doc, f, separators=(",", ":"))
    os.replace(tmp, path)          # 先寫再改名，讀的人不會看到半個檔案
    return path


def prune():
    cutoff = (datetime.now(timezone.utc) - timedelta(days=RETENTION_DAYS)).strftime("%Y-%m-%d")
    for name in sorted(os.listdir(OUT_DIR)):
        full = f"{OUT_DIR}/{name}"
        if not os.path.isdir(full) or len(name) != 10 or name >= cutoff:
            continue
        for f in os.listdir(full):
            os.remove(f"{full}/{f}")
        os.rmdir(full)
        log(f"prune 刪除 {name}")


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    log(f"啟動：間隔 {INTERVAL}s，部門標籤 {DEPT_LABEL}，輸出 {OUT_DIR}，保留 {RETENTION_DAYS} 天")
    last_prune = ""
    while True:
        started = time.time()
        try:
            rows = collect()
            path = write_snapshot(rows)
            with_dept = sum(1 for r in rows if r["dept"])
            log(f"快照 {os.path.basename(path)}：{len(rows)} 個 Pod，{with_dept} 個有部門標籤，"
                f"宣告 {sum(r['cpu'] for r in rows):.3f} 核 / "
                f"{sum(r['mem'] for r in rows) / 2**30:.2f} GiB")
            # 心跳檔給 liveness 探針看。探「行程還活著」沒有意義——真正要抓的是
            # 「行程還活著但已經寫不出快照」，那才是會無聲吃掉分攤依據的狀況。
            with open(f"{OUT_DIR}/.heartbeat", "w") as f:
                f.write(datetime.now(timezone.utc).isoformat())
            today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
            if today != last_prune:
                prune()
                last_prune = today
        except Exception as e:                       # 取樣失敗不該讓整支程式死掉
            log(f"錯誤：{type(e).__name__}: {e}")
        time.sleep(max(1, INTERVAL - (time.time() - started)))


if __name__ == "__main__":
    args = sys.argv[1:]
    ONCE = "--once" in args
    if "--kubectl" in args:
        i = args.index("--kubectl")
        KUBECTL = args[i + 1] if len(args) > i + 1 and not args[i + 1].startswith("-") else "1"
    if ONCE:
        os.makedirs(OUT_DIR, exist_ok=True)
        rows = collect()
        log(f"單次快照 {write_snapshot(rows)}：{len(rows)} 個 Pod")
        sys.exit(0)
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
