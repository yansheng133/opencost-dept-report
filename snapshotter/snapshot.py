#!/usr/bin/env python3
"""宣告量快照器：定期把每個 Pod 的資源宣告量與部門標籤存下來。

為什麼需要這個：Prometheus 掛掉的時候，成本還是在發生——節點照跑、雲端照計費、
機器照折舊——但你失去了「這些錢該分給誰」的依據。這支程式只跟 API server 講話，
完全不碰 Prometheus，所以兩者不會同時消失。真的斷線時，這些快照就是第二份分攤依據。

它刻意做得很笨：沒有資料庫、沒有相依套件、一個迴圈把 JSON 寫到磁碟。
備援系統比它保護的系統更複雜的話，就失去意義了。

除了寫檔，它還開一個很小的唯讀 HTTP 介面（`/allocation`、`/healthz`），
讓同叢集的 cost-report 發現資料斷層時可以即時問「這段區間各部門宣告了多少」，
不必等人去 kubectl cp。查詢用的算式跟 fallback.py 是同一份程式碼（見下方
load_snapshots／allocate／build_allocation），離線算跟線上問不會給出兩種答案。

環境變數：
  OUT_DIR=/data          快照存放目錄
  INTERVAL=300           取樣間隔（秒）
  DEPT_LABEL=cost-center 部門標籤的鍵
  ENV_LABEL=env          第二維度標籤的鍵
  RETENTION_DAYS=35      保留天數（一個帳期加緩衝）
  LISTEN_PORT=8080       查詢介面的埠；設成 0 就不開
"""
import json
import os
import ssl
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SA = "/var/run/secrets/kubernetes.io/serviceaccount"
OUT_DIR = os.environ.get("OUT_DIR", "/data")
INTERVAL = int(os.environ.get("INTERVAL", "300"))
DEPT_LABEL = os.environ.get("DEPT_LABEL", "cost-center")
ENV_LABEL = os.environ.get("ENV_LABEL", "env")
RETENTION_DAYS = int(os.environ.get("RETENTION_DAYS", "35"))
LISTEN_PORT = int(os.environ.get("LISTEN_PORT", "8080"))
# schema 2 起多了 pvcs（每個 PVC 一列的儲存宣告量）與 Pod 層的 pv 欄位。
# 讀的那端要同時吃得下 1 與 2——舊快照還躺在 PVC 裡，保留 35 天，
# 升級當天就把它們讀壞的話，等於自己把備援資料刪了一個帳期。
SCHEMA = 2
GIB = 2**30
KUBECTL = ""          # 空字串＝叢集內模式；否則是 kubeconfig 路徑（或 "1" 用預設 context）


def log(msg):
    print(f"{datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')} {msg}", flush=True)


def api(path):
    """取資料。兩種來源：叢集內用 ServiceAccount，叢集外用本機的 kubectl。

    kubectl 模式是給「部署之前先看看它會抓到什麼」用的，欄位與叢集內完全一樣，
    所以拿它收的快照跟正式的可以混著算。
    """
    if KUBECTL:
        if "persistentvolumeclaims" in path:
            args = ["get", "persistentvolumeclaims", "--all-namespaces", "-o", "json"]
        elif path.startswith("/api/v1/namespaces") and "pods" not in path:
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


def claimed_storage(spec, pvc_bytes, ns):
    """這個 Pod 掛了多少儲存（位元組）。

    同一個 PVC 在同一個 Pod 裡被掛成兩個 volume 是合法的（不同 subPath），
    但那仍然只有一份儲存，所以先用 claimName 去重再加總。
    """
    claims = set()
    for v in spec.get("volumes", []) or []:
        pvc = v.get("persistentVolumeClaim") or {}
        if pvc.get("claimName"):
            claims.add(pvc["claimName"])
    return sum(pvc_bytes.get((ns, c), 0) for c in claims)


def collect():
    """取一次快照：所有 Running 的 Pod、所有 PVC，配上它們該記到哪個部門。

    回傳 (pod 列表, pvc 列表)。兩份分開存不是為了好看：儲存的成本屬於 PVC，
    不屬於掛它的 Pod。兩個 Pod 掛同一個 PVC 時，磁碟只有一份、帳也只該收一份，
    所以部門分攤一律用 pvcs 那一份算，Pod 上的 pv 只拿來查「誰掛了什麼」。
    """
    ns_labels = {}
    for ns in api("/api/v1/namespaces").get("items", []):
        ns_labels[ns["metadata"]["name"]] = ns["metadata"].get("labels") or {}

    pvc_rows, pvc_bytes = [], {}
    for c in api("/api/v1/persistentvolumeclaims").get("items", []):
        meta = c["metadata"]
        ns = meta["namespace"]
        labels = meta.get("labels") or {}
        nsl = ns_labels.get(ns, {})
        size = mem_bytes(((c.get("spec", {}).get("resources", {}) or {})
                          .get("requests", {}) or {}).get("storage"))
        pvc_bytes[(ns, meta["name"])] = int(size)
        # 部門歸屬跟 Pod 同一套規則：物件自己的標籤優先，沒有才退回 namespace 的。
        # 不去看「掛它的 Pod 是哪個部門的」——那樣同一個 PVC 會有兩種歸屬。
        pvc_rows.append({
            "ns": ns, "name": meta["name"],
            "dept": labels.get(DEPT_LABEL) or nsl.get(DEPT_LABEL) or "",
            "env": labels.get(ENV_LABEL) or nsl.get(ENV_LABEL) or "",
            "bytes": int(size),
        })

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
            "pv": claimed_storage(spec, pvc_bytes, ns),
            "node": spec.get("nodeName", ""), "owner": owner_of(p),
        })
    return rows, pvc_rows


# ── 讀回來算分攤：fallback.py 與下方的查詢介面共用這一段 ─────────────────
# 放在這裡而不是各寫一份，是因為「離線 kubectl cp 之後算的數字」跟「cost-report
# 線上問到的數字」必須是同一個。兩份實作遲早會漂移，而漂移的症狀是兩張帳單對不起來。

def parse_ts(s):
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)


def load_snapshots(snap_dir, start, end):
    """讀出區間內的快照（start 含、end 不含）。壞掉的檔案跳過，不要讓一個壞檔擋住整段。"""
    snaps = []
    for day in sorted(os.listdir(snap_dir)):
        d = os.path.join(snap_dir, day)
        if not os.path.isdir(d) or len(day) != 10:
            continue
        for name in sorted(os.listdir(d)):
            if not name.endswith(".json"):
                continue
            try:
                with open(os.path.join(d, name)) as f:
                    doc = json.load(f)
                ts = parse_ts(doc["ts"])
            except (ValueError, KeyError, json.JSONDecodeError):
                continue
            if start <= ts < end:
                snaps.append(doc)
    snaps.sort(key=lambda s: s["ts"])
    return snaps


def allocate(snaps, interval_s):
    """每一張快照代表 interval 這段時間的狀態，乘起來就是宣告的資源小時數。

    儲存只看 snap["pvcs"]，不看 Pod 的 pv 欄位：pv 是「這個 Pod 掛了多少」，
    多個 Pod 掛同一個 PVC 時加總會重複計算，而磁碟只有一份。
    schema 1 的舊快照沒有 pvcs，當成 0——當時確實沒有收這筆資料，
    補一個猜的數字進去比留白更糟。
    """
    hours = interval_s / 3600.0
    by_dept = {}
    totals = {"cpu_h": 0.0, "ram_gib_h": 0.0, "pv_gib_h": 0.0}

    def row(dept):
        return by_dept.setdefault(dept or "__unallocated__",
                                  {"cpu_h": 0.0, "ram_gib_h": 0.0, "pv_gib_h": 0.0,
                                   "pods": set()})

    for snap in snaps:
        for p in snap.get("pods", []):
            r = row(p["dept"])
            r["cpu_h"] += p["cpu"] * hours
            r["ram_gib_h"] += p["mem"] / GIB * hours
            r["pods"].add(f"{p['ns']}/{p['owner'] or p['pod']}")
            totals["cpu_h"] += p["cpu"] * hours
            totals["ram_gib_h"] += p["mem"] / GIB * hours
        for c in snap.get("pvcs") or []:
            r = row(c["dept"])
            r["pv_gib_h"] += c["bytes"] / GIB * hours
            totals["pv_gib_h"] += c["bytes"] / GIB * hours
    return by_dept, totals


def build_allocation(snap_dir, start, end):
    """區間查詢的完整結果，找不到任何快照時回傳 None。

    回 None 而不是回一份全零的結果，是因為呼叫端必須分得出「沒有資料」和
    「資料是零」。前者要往降級階梯再下一階，後者可以直接出帳。
    """
    snaps = load_snapshots(snap_dir, start, end)
    if not snaps:
        return None
    interval_s = snaps[0].get("interval_s", 300)
    # 快照自己也可能有斷層（這支備援掛掉的時候）。完整度要先講清楚，
    # 不然用一份殘缺的備援去補另一份殘缺的資料，錯得更難發現。
    expected = max(1, round((end - start).total_seconds() / interval_s))
    by_dept, totals = allocate(snaps, interval_s)
    return {
        "snaps": snaps,
        "interval_s": interval_s,
        "expected": expected,
        # 上限 100：取樣時間點跟區間邊界對不齊時，找到的筆數會比期望多一筆，
        # 而「完整度 105%」只會讓看的人不知道該信哪個數字。
        "coverage": min(100.0, len(snaps) / expected * 100),
        "by_dept": by_dept,
        "totals": totals,
    }


def departments_json(by_dept):
    """把 allocate() 的結果轉成對外的欄位名稱。cost-report 照這個寫，不要隨便改。"""
    return {d: {"cpuCoreHours": round(r["cpu_h"], 4),
                "ramGiBHours": round(r["ram_gib_h"], 4),
                "pvGiBHours": round(r["pv_gib_h"], 4),
                "workloads": len(r["pods"])}
            for d, r in by_dept.items()}


# ── 查詢介面 ────────────────────────────────────────────────────────────

class _Handler(BaseHTTPRequestHandler):
    """唯讀查詢介面。只讀磁碟上已經寫好的快照，不會去碰 API server，
    所以被打爆也只是自己變慢，不會連累取樣迴圈或 API server。
    """

    def log_message(self, fmt, *args):
        # readinessProbe 每幾秒打一次 /healthz，照預設印出來會把真正的取樣日誌淹掉
        pass

    def _send(self, code, body, ctype):
        raw = body.encode() if isinstance(body, str) else body
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _json(self, code, obj):
        self._send(code, json.dumps(obj, ensure_ascii=False), "application/json")

    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        if u.path == "/healthz":
            return self._send(200, "ok", "text/plain; charset=utf-8")
        if u.path != "/allocation":
            return self._json(404, {"error": f"不認得的路徑：{u.path}"})

        q = urllib.parse.parse_qs(u.query)
        try:
            start = parse_ts(q.get("start", [""])[0])
            end = parse_ts(q.get("end", [""])[0])
        except ValueError:
            return self._json(400, {"error": "start/end 要是 RFC3339 UTC，像 2026-09-24T05:00:00Z"})
        if start >= end:
            return self._json(400, {"error": "start 要早於 end"})

        try:
            res = build_allocation(OUT_DIR, start, end)
        except OSError as e:                     # 快照目錄還沒建立、或磁碟出問題
            return self._json(500, {"error": f"{type(e).__name__}: {e}"})
        if res is None:
            # 404 不是錯誤，是「這段區間我沒有資料」。回 200 加一份空結果的話，
            # cost-report 會把它當成「這段時間成本是零」，然後無聲地少收一段錢。
            return self._json(404, {"error": f"這段區間沒有任何快照："
                                             f"{start:%Y-%m-%dT%H:%M:%SZ} ~ {end:%Y-%m-%dT%H:%M:%SZ}"})

        self._json(200, {
            "window": {"start": f"{start:%Y-%m-%dT%H:%M:%SZ}", "end": f"{end:%Y-%m-%dT%H:%M:%SZ}"},
            "basis": "declared",
            "snapshotCoveragePct": round(res["coverage"], 2),
            "samples": {"found": len(res["snaps"]), "expected": res["expected"],
                        "intervalSeconds": res["interval_s"]},
            "departments": departments_json(res["by_dept"]),
        })


def serve(port):
    """在背景執行緒開查詢介面。

    取樣迴圈是這支程式的本業，查詢只是附加的，所以用 daemon 執行緒：
    主迴圈要結束的時候不該被一個還沒關掉的連線卡住。
    開不起來（埠被佔、沒權限）就記一行繼續跑——查不到總比連快照都不收好。
    """
    if port <= 0:
        return None
    try:
        srv = ThreadingHTTPServer(("", port), _Handler)
    except OSError as e:
        log(f"查詢介面開不起來（port {port}）：{type(e).__name__}: {e}；取樣照常進行")
        return None
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    log(f"查詢介面：http://0.0.0.0:{port}/allocation　健康檢查 /healthz")
    return srv


def write_snapshot(rows, pvc_rows):
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
        "pvcs": pvc_rows,
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
    serve(LISTEN_PORT)
    last_prune = ""
    while True:
        started = time.time()
        try:
            rows, pvc_rows = collect()
            path = write_snapshot(rows, pvc_rows)
            with_dept = sum(1 for r in rows if r["dept"])
            log(f"快照 {os.path.basename(path)}：{len(rows)} 個 Pod，{with_dept} 個有部門標籤，"
                f"宣告 {sum(r['cpu'] for r in rows):.3f} 核 / "
                f"{sum(r['mem'] for r in rows) / GIB:.2f} GiB / "
                f"{len(pvc_rows)} 個 PVC 共 {sum(c['bytes'] for c in pvc_rows) / GIB:.2f} GiB")
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
        rows, pvc_rows = collect()
        log(f"單次快照 {write_snapshot(rows, pvc_rows)}："
            f"{len(rows)} 個 Pod，{len(pvc_rows)} 個 PVC")
        sys.exit(0)
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
