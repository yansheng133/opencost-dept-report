#!/usr/bin/env python3
"""local-path PVC 的實際用量：在節點上量每個 PVC 目錄佔了多少磁碟，給 Prometheus 抓。

為什麼要有這支：OpenCost 照 PVC **申請的容量**收費，要看「申請了多少、實際用了多少」，
實際用量得另外拿。正常的來源是 kubelet 的 kubelet_volume_stats_used_bytes，
但 kubelet 只量 CSI 磁碟區——local-path 建出來的 PV 是 hostPath（節點上的一個目錄），
kubelet 完全不量。實測 kubelet 的 /metrics 與 /stats/summary 裡都沒有這幾個 PVC。
**這種缺漏是無聲的：查詢不會出錯，只是回傳空的。**

做法跟 du 一樣：走過 local-path 根目錄底下每個 PVC 目錄，加總實際佔用的區塊。
只讀不寫；以 DaemonSet 在每個節點上跑，各量各的。

換成 Longhorn 或其他 CSI 儲存之後，kubelet 自己就有這個數字，這支就不需要了。

    python3 volume_usage.py            VOLUME_ROOT=/volumes SCAN_INTERVAL=600 LISTEN_PORT=9102
"""
import os
import re
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.environ.get("VOLUME_ROOT", "/volumes")
# 十分鐘一次：磁碟用量變化慢，而 du 要走過每一個檔案，太頻繁會跟工作負載搶 I/O
INTERVAL = int(os.environ.get("SCAN_INTERVAL", "600"))
PORT = int(os.environ.get("LISTEN_PORT", "9102"))
NODE = os.environ.get("NODE_NAME", "")

# local-path 預設的目錄名稱：<PV 名稱>_<namespace>_<PVC 名稱>。
# namespace 與 PVC 名稱都不能有底線（Kubernetes 的命名規則），所以用底線切開不會切錯。
# 改過 local-path 的 pathPattern 的話，目錄名稱就不是這個樣子，會被算進 unparsed。
DIR_RE = re.compile(r"^(pvc-[0-9a-f-]{36})_([a-z0-9]([a-z0-9-]*[a-z0-9])?)_([a-z0-9]([a-z0-9.-]*[a-z0-9])?)$")


def parse_dir(name):
    """目錄名稱 → (PV, namespace, PVC)；不是 local-path 建的目錄就回 None。"""
    m = DIR_RE.match(name)
    return (m.group(1), m.group(2), m.group(4)) if m else None


def disk_usage(path):
    """回傳 (位元組, 檔案數, 讀不到的項目數)。

    算的是**實際佔用的區塊**（st_blocks），跟 du 一樣，不是檔案長度：
    稀疏檔案的長度可以遠大於它真的佔的空間，拿長度算會高估。
    硬連結只算一次；不跟著符號連結走；不跨進別的檔案系統（目錄裡如果另外掛了東西，那不是這個 PVC 的）。
    讀不到的項目要回報出去——**少算是無聲的**，看的人必須知道這個數字可能偏低。
    """
    try:
        top = os.lstat(path)
    except OSError:
        return 0, 0, 1
    total, files, errors = top.st_blocks * 512, 0, 0
    seen = set()
    stack = [path]
    while stack:
        p = stack.pop()
        try:
            it = os.scandir(p)
        except OSError:
            errors += 1
            continue
        with it:
            for e in it:
                try:
                    st = e.stat(follow_symlinks=False)
                    is_dir = e.is_dir(follow_symlinks=False)
                except OSError:
                    errors += 1
                    continue
                if st.st_dev != top.st_dev:
                    continue
                if not is_dir and st.st_nlink > 1:
                    key = (st.st_dev, st.st_ino)
                    if key in seen:
                        continue
                    seen.add(key)
                total += st.st_blocks * 512
                files += 1
                if is_dir:
                    stack.append(e.path)
    return total, files, errors


def scan(root):
    """量一次。回傳 {"volumes": [...], "unparsed": n, "duration": 秒, "at": 時間}。"""
    t0 = time.time()
    vols, unparsed = [], 0
    try:
        names = sorted(os.listdir(root))
    except OSError as e:
        print(f"讀不到 {root}：{e}", file=sys.stderr)
        names = None
    for name in names or []:
        full = os.path.join(root, name)
        if not os.path.isdir(full) or os.path.islink(full):
            continue
        parsed = parse_dir(name)
        if not parsed:
            unparsed += 1
            continue
        used, files, errors = disk_usage(full)
        vols.append({"pv": parsed[0], "ns": parsed[1], "pvc": parsed[2],
                     "used": used, "files": files, "errors": errors})
    return {"volumes": vols, "unparsed": unparsed, "rootOk": names is not None,
            "duration": time.time() - t0, "at": time.time()}


def render(result, node=""):
    """Prometheus 文字格式。標籤不用 namespace：抓取設定會把 Pod 自己的 namespace
    放進同名標籤（而且這裡開了 honor_labels），撞名的話兩邊都會被搞混。"""
    out = []

    def metric(name, help_, rows):
        out.append(f"# HELP {name} {help_}")
        out.append(f"# TYPE {name} gauge")
        out.extend(rows)

    nl = f'node="{node}"' if node else ""
    nb = "{" + nl + "}" if nl else ""      # 只有節點標籤的那幾個 metric

    def lab(v):
        parts = [f'pv="{v["pv"]}"', f'pvc_namespace="{v["ns"]}"', f'pvc="{v["pvc"]}"']
        return "{" + ",".join(parts + ([nl] if nl else [])) + "}"

    if result is None:
        metric("cost_report_volume_scan_ok", "上一次掃描是否成功（還沒掃完第一次時是 0）",
               [f"cost_report_volume_scan_ok{nb} 0"])
        return "\n".join(out) + "\n"
    vols = result["volumes"]
    metric("cost_report_pvc_used_bytes",
           "PVC 目錄實際佔用的磁碟空間（位元組，跟 du 一樣算區塊）",
           [f"cost_report_pvc_used_bytes{lab(v)} {v['used']}" for v in vols])
    metric("cost_report_pvc_scan_errors",
           "量這個 PVC 時讀不到的項目數；大於 0 表示用量可能偏低",
           [f"cost_report_pvc_scan_errors{lab(v)} {v['errors']}" for v in vols])
    metric("cost_report_volume_unparsed_dirs",
           "根目錄底下認不出是哪個 PVC 的目錄數（local-path 的 pathPattern 改過時會大於 0）",
           [f"cost_report_volume_unparsed_dirs{nb} {result['unparsed']}"])
    metric("cost_report_volume_scan_duration_seconds", "上一次掃描花了幾秒",
           [f"cost_report_volume_scan_duration_seconds{nb} {result['duration']:.3f}"])
    metric("cost_report_volume_scan_timestamp_seconds", "上一次掃描完成的時間（Unix 秒）",
           [f"cost_report_volume_scan_timestamp_seconds{nb} {result['at']:.0f}"])
    metric("cost_report_volume_scan_ok", "上一次掃描是否成功（還沒掃完第一次時是 0）",
           [f"cost_report_volume_scan_ok{nb} {1 if result['rootOk'] else 0}"])
    return "\n".join(out) + "\n"


_last = None


def scanner():
    global _last
    while True:
        r = scan(ROOT)
        _last = r
        bad = sum(v["errors"] for v in r["volumes"])
        print(f"掃描完成：{len(r['volumes'])} 個 PVC、{r['duration']:.1f} 秒"
              + (f"、{bad} 個項目讀不到" if bad else "")
              + (f"、{r['unparsed']} 個目錄認不出來" if r["unparsed"] else ""), flush=True)
        time.sleep(INTERVAL)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):   # Prometheus 每分鐘來一次，不必每次都印
        pass

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/metrics":
            body, code = render(_last, NODE).encode(), 200
            ctype = "text/plain; version=0.0.4; charset=utf-8"
        elif path == "/healthz":
            body, code, ctype = b"ok\n", 200, "text/plain"
        elif path == "/readyz":
            ready = _last is not None
            body, code, ctype = (b"ok\n", 200, "text/plain") if ready else (b"scanning\n", 503, "text/plain")
        else:
            body, code, ctype = b"not found\n", 404, "text/plain"
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    try:
        os.nice(10)       # 讓出 CPU 給工作負載；調低優先權不需要任何權限
    except OSError:
        pass
    threading.Thread(target=scanner, daemon=True).start()
    print(f"volume-usage：量 {ROOT}，每 {INTERVAL} 秒一次，:{PORT}/metrics", flush=True)
    ThreadingHTTPServer(("", PORT), Handler).serve_forever()


if __name__ == "__main__":
    main()
