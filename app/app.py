#!/usr/bin/env python3
"""部門成本分攤報表：在叢集內查 OpenCost，快取後提供網頁與 JSON。

設定（環境變數）：
  OPENCOST_URL   OpenCost API 位址（預設 http://opencost.opencost.svc.cluster.local:9003）
  CACHE_TTL      快取秒數（預設 60）
  WINDOWS        允許的查詢區間，逗號分隔（預設 1h,24h,7d）
  DEFAULT_WINDOW 預設區間（預設 24h）
  LISTEN_PORT    監聽埠（預設 8080）
  CLUSTER_LABEL  顯示用的叢集名稱

端點：
  GET /                 報表網頁
  GET /api/report?window=24h   JSON（快取）
  GET /healthz          活著就回 200
  GET /readyz           至少成功抓過一次資料才回 200

設計重點：
  - 只讀 OpenCost 的 allocation API，不碰 Kubernetes API，所以不需要任何 RBAC。
  - 背景執行緒定期更新；OpenCost 掛掉時沿用上一份資料並標記 stale，不讓畫面變空白。
  - 單價由資料反推（金額 ÷ 用量），不寫死，換單價不必改程式。
  - 這支服務沒有登入機制：誰連得到就看得到全部部門。要限制存取，認證要放在它前面。
"""
import calendar
import json
import os
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

OPENCOST_URL = os.environ.get("OPENCOST_URL", "http://opencost.opencost.svc.cluster.local:9003").rstrip("/")
CACHE_TTL = int(os.environ.get("CACHE_TTL", "60"))
WINDOWS = [w.strip() for w in os.environ.get("WINDOWS", "1h,24h,7d").split(",") if w.strip()]
DEFAULT_WINDOW = os.environ.get("DEFAULT_WINDOW", "24h")
LISTEN_PORT = int(os.environ.get("LISTEN_PORT", "8080"))
CLUSTER_LABEL = os.environ.get("CLUSTER_LABEL", "")
HTTP_TIMEOUT = int(os.environ.get("HTTP_TIMEOUT", "30"))
UI_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "index.html")
# 量測覆蓋率要直接問 Prometheus（OpenCost 自己不會說「這段我沒量到」）。連不上就只是少一塊資訊。
PROM_URL = os.environ.get("PROM_URL", "http://prometheus-server.prometheus-system.svc.cluster.local:80").rstrip("/")
COVERAGE_JOB = os.environ.get("COVERAGE_JOB", "opencost")
# 覆蓋率只是加值資訊，逾時要短。位址設錯的話，用主逾時（30 秒）會讓每次更新多卡一分鐘，
# 變成「為了知道資料完不完整，反而拖慢了資料本身」。
PROM_TIMEOUT = int(os.environ.get("PROM_TIMEOUT", "5"))

DEPT_KEYS = ("mfg", "rd", "it")          # 只是預設順序；實際部門以資料為準
SYSTEM_NS = [p.strip() for p in os.environ.get(
    "SYSTEM_NS_PREFIXES",
    "kube-system,kube-public,kube-node-lease,cattle-,prometheus-system,opencost,local-path-storage,compliance-operator-system,cost-report"
).split(",") if p.strip()]   # 這些 namespace 的元件不列進部門報表
GIB = 1024 ** 3
MIB = 1024 ** 2

_cache = {}          # window -> {"data": ..., "fetched": ts, "stale": bool, "error": str|None}
_lock = threading.Lock()
_ready = threading.Event()


def _get(path, params):
    url = f"{OPENCOST_URL}{path}?{urllib.parse.urlencode(params)}"
    req = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=HTTP_TIMEOUT) as r:
        body = json.load(r)
    if body.get("code") != 200:
        raise RuntimeError(f"OpenCost 回應 code={body.get('code')} message={body.get('message')}")
    data = body.get("data") or [{}]
    return data[0] or {}


def _alloc(window, aggregate, **extra):
    params = {"window": window, "aggregate": aggregate, "accumulate": "true"}
    params.update(extra)
    return _get("/allocation/compute", params)


def _round(v, n=4):
    try:
        return round(float(v), n)
    except (TypeError, ValueError):
        return 0.0


def _prom(path, params):
    url = f"{PROM_URL}{path}?{urllib.parse.urlencode(params)}"
    with urllib.request.urlopen(url, timeout=PROM_TIMEOUT) as r:
        body = json.load(r)
    if body.get("status") != "success":
        raise RuntimeError(f"Prometheus 回應 {body.get('status')}: {body.get('error')}")
    return body.get("data") or {}


def _scrape_interval():
    """從 Prometheus 的 target 直接問抓取間隔，不要用猜的。

    猜錯的話覆蓋率會系統性地偏高或偏低，而且不會有任何徵兆——
    帳單上「99% 完整」如果其實是 66%，比沒有這個數字更糟。
    """
    try:
        for t in _prom("/api/v1/targets", {"state": "active"}).get("activeTargets", []):
            if t.get("labels", {}).get("job") == COVERAGE_JOB:
                s = t.get("scrapeInterval", "")
                if s.endswith("s"):
                    return float(s[:-1])
                if s.endswith("m"):
                    return float(s[:-1]) * 60
    except (urllib.error.URLError, OSError, RuntimeError, ValueError, KeyError):
        pass
    return None


def coverage(start_iso, end_iso):
    """這段區間到底有多少時間真的量到了，缺的又缺在哪裡。

    沒有這個數字，缺漏就是無聲的：報表照樣產出，只是少算了一些錢，
    而且沒有人會發現。有了它，才談得上「要不要用推估補、補多少」。
    """
    if not PROM_URL:
        return None
    try:
        start = _parse_iso(start_iso)
        end = _parse_iso(end_iso)
    except (TypeError, ValueError):
        return None
    span = end - start
    if span <= 0:
        return None
    step = _scrape_interval()
    measured_step = step is not None
    if step is None:
        step = 60.0
    # 點數上限：7 天 × 60 秒會是一萬多點，沒必要。放粗只會漏掉比 step 更短的斷層。
    step = max(step, span / 1500)
    try:
        data = _prom("/api/v1/query_range", {
            "query": f'up{{job="{COVERAGE_JOB}"}}', "start": start, "end": end, "step": int(step)})
        series = data.get("result") or []
    except (urllib.error.URLError, OSError, RuntimeError, ValueError) as e:
        return {"available": False, "reason": str(e)}

    expected = int(span // step) + 1
    seen = {}
    for s in series:
        for ts, val in s.get("values", []):
            if val == "1":      # 同一個 job 可能有多個 target，任何一個有回報就算量到了
                seen[int((float(ts) - start) // step)] = True
    up = len(seen)

    gaps, run_start = [], None
    for i in range(expected):
        missing = not seen.get(i, False)
        if missing and run_start is None:
            run_start = i
        elif not missing and run_start is not None:
            gaps.append((run_start, i))
            run_start = None
    if run_start is not None:
        gaps.append((run_start, expected))

    def iso(idx):
        return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(start + idx * step))

    return {
        "available": True,
        "pct": _round(up / expected * 100, 2) if expected else 0.0,
        "measuredStep": measured_step,          # False = 沒問到抓取間隔，用了預設值 60s
        "stepSeconds": int(step),
        "samples": {"found": up, "expected": expected},
        # 只列最大的幾段：帳單上要的是「缺在哪、有多久」，不是逐點清單
        "gaps": [{"start": iso(a), "end": iso(b), "minutes": _round((b - a) * step / 60, 1)}
                 for a, b in sorted(gaps, key=lambda g: g[0] - g[1])[:5]],
        "gapMinutes": _round(sum((b - a) * step for a, b in gaps) / 60, 1),
    }


def _parse_iso(s):
    """OpenCost 的時間字串換成 epoch 秒。用 timegm，不要用 mktime——後者會套上本機時區，
    容器是 UTC、你的筆電是 UTC+8，差 8 小時的斷層報告完全看不出來是時區問題。"""
    txt = s.strip().replace("+0000", "Z").replace("+00:00", "Z")
    if "." in txt:
        txt = txt.split(".")[0] + "Z"
    return calendar.timegm(time.strptime(txt, "%Y-%m-%dT%H:%M:%SZ"))


def build_report(window):
    """把 OpenCost 的四個查詢整理成畫面要用的形狀。"""
    by_cc = _alloc(window, "label:cost-center", includeIdle="true", shareIdle="false")
    by_cc_shared = _alloc(window, "label:cost-center", includeIdle="true", shareIdle="true")
    by_cc_env = _alloc(window, "label:cost-center,label:env")
    by_ctrl = _alloc(window, "controller,label:cost-center,label:env")
    by_ns = _alloc(window, "namespace")

    idle = _round(by_cc.get("__idle__", {}).get("totalCost", 0))
    unalloc_sep = _round(by_cc.get("__unallocated__", {}).get("totalCost", 0))
    unalloc_shr = _round(by_cc_shared.get("__unallocated__", {}).get("totalCost", 0))

    depts = []
    for key, v in by_cc.items():
        if key.startswith("__"):
            continue
        depts.append({
            "id": key,
            "cpu": _round(v.get("cpuCost")), "ram": _round(v.get("ramCost")), "pv": _round(v.get("pvCost")),
            "total": _round(v.get("totalCost")),
            "shared": _round(by_cc_shared.get(key, {}).get("totalCost", 0)),
            "cpuCoreHours": _round(v.get("cpuCoreHours")),
            "ramGiBHours": _round((v.get("ramByteHours") or 0) / GIB),
            "pvGiBHours": _round((v.get("pvByteHours") or 0) / GIB),
        })
    order = {k: i for i, k in enumerate(DEPT_KEYS)}
    depts.sort(key=lambda d: (order.get(d["id"], 99), d["id"]))

    env_rows = {}
    for key, v in by_cc_env.items():
        cc, _, env = key.partition("/")
        if cc.startswith("__"):
            continue
        row = env_rows.setdefault(cc, {"dept": cc, "prod": 0.0, "dev": 0.0, "other": 0.0})
        slot = env if env in ("prod", "dev") else "other"
        row[slot] = _round(row[slot] + float(v.get("totalCost") or 0))

    workloads = []
    for key, v in by_ctrl.items():
        name = (v.get("name") or key).split("/")[0]
        if not name.startswith("deployment:") and not name.startswith("statefulset:") and not name.startswith("daemonset:"):
            continue
        ns = (v.get("properties") or {}).get("namespace") or ""
        has_dept = len(key.split("/")) > 2 and not key.split("/")[1].startswith("__")
        # 系統 namespace 的元件不列進部門報表；有部門標籤的一律保留（跨部門計費會用到）
        if not has_dept and any(ns == p or ns.startswith(p) for p in SYSTEM_NS):
            continue
        parts = key.split("/")
        cc = parts[1] if len(parts) > 2 else None
        env = parts[2] if len(parts) > 2 else None
        props = v.get("properties") or {}
        workloads.append({
            "name": name.split(":", 1)[1],
            "kind": name.split(":", 1)[0],
            "ns": props.get("namespace") or "",
            "dept": None if not cc or cc.startswith("__") else cc,
            "env": None if not env or env.startswith("__") else env,
            "total": _round(v.get("totalCost")),
            "cpuReq": _round(v.get("cpuCoreRequestAverage"), 3), "cpuUse": _round(v.get("cpuCoreUsageAverage"), 3),
            "ramReqMi": round((v.get("ramByteRequestAverage") or 0) / MIB),
            "ramUseMi": round((v.get("ramByteUsageAverage") or 0) / MIB),
            "cpuEff": _round(v.get("cpuEfficiency"), 3), "ramEff": _round(v.get("ramEfficiency"), 3),
        })
    workloads.sort(key=lambda w: -w["total"])

    dept_ns = {w["ns"] for w in workloads if w["dept"]}
    unalloc_rows = sorted(
        ({"ns": k, "total": _round(v.get("totalCost"))} for k, v in by_ns.items()
         if not k.startswith("__") and k not in dept_ns),
        key=lambda r: -r["total"])

    # 單價由資料反推：金額 ÷ 用量。避免在兩個地方各寫一次單價。
    # 只能用有標籤的部門來反推：__idle__ 有金額但用量欄位是 0，混進來會把單價灌高。
    def implied(cost_key, usage_key, div=1.0):
        rows = [v for k, v in by_cc.items() if not k.startswith("__")]
        cost = sum(float(r.get(cost_key) or 0) for r in rows)
        usage = sum(float(r.get(usage_key) or 0) for r in rows) / div
        return _round(cost / usage, 6) if usage else None

    any_dept = next((v for k, v in by_cc.items() if not k.startswith("__")), {})
    total = _round(sum(float(v.get("totalCost") or 0) for v in by_cc.values()))
    return {
        "window": {
            "query": window,
            "start": any_dept.get("start"), "end": any_dept.get("end"),
            "minutes": _round(any_dept.get("minutes"), 1),
        },
        "cluster": CLUSTER_LABEL,
        "coverage": coverage(any_dept.get("start"), any_dept.get("end")),
        "rates": {
            "cpu": implied("cpuCost", "cpuCoreHours"),
            "ram": implied("ramCost", "ramByteHours", GIB),
            "storage": implied("pvCost", "pvByteHours", GIB),
        },
        "depts": depts, "idle": idle,
        "unallocated": {"separate": unalloc_sep, "shared": unalloc_shr},
        "total": total,
        "ccEnv": [env_rows[k] for k in sorted(env_rows, key=lambda k: order.get(k, 99))],
        "workloads": workloads, "unallocRows": unalloc_rows,
    }


def refresh(window):
    try:
        data = build_report(window)
        with _lock:
            _cache[window] = {"data": data, "fetched": time.time(), "stale": False, "error": None}
        _ready.set()
        return True
    except (urllib.error.URLError, OSError, RuntimeError, ValueError) as e:
        with _lock:
            prev = _cache.get(window)
            if prev:
                prev["stale"] = True
                prev["error"] = str(e)
            else:
                _cache[window] = {"data": None, "fetched": 0, "stale": True, "error": str(e)}
        print(f"[warn] 更新 {window} 失敗：{e}", flush=True)
        return False


def refresher():
    while True:
        for w in WINDOWS:
            refresh(w)
        time.sleep(CACHE_TTL)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):   # 預設會把每個請求印到 stderr，這裡精簡成一行
        print(f'{self.address_string()} "{self.requestline}" {args[1] if len(args) > 1 else ""}', flush=True)

    def _send(self, code, body, ctype, extra=None):
        payload = body if isinstance(body, bytes) else body.encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        path, _, query = self.path.partition("?")
        qs = urllib.parse.parse_qs(query)

        if path == "/healthz":
            return self._send(200, "ok\n", "text/plain; charset=utf-8")
        if path == "/readyz":
            return self._send(200 if _ready.is_set() else 503,
                              "ready\n" if _ready.is_set() else "waiting for first fetch\n",
                              "text/plain; charset=utf-8")
        if path == "/api/report":
            window = (qs.get("window") or [DEFAULT_WINDOW])[0]
            if window not in WINDOWS:
                return self._send(400, json.dumps({"error": f"window 必須是 {WINDOWS} 其中之一"}, ensure_ascii=False),
                                  "application/json; charset=utf-8")
            with _lock:
                entry = dict(_cache.get(window) or {})
            if not entry.get("data") and not refresh(window):
                with _lock:
                    entry = dict(_cache.get(window) or {})
                return self._send(503, json.dumps({"error": entry.get("error") or "尚未取得資料"}, ensure_ascii=False),
                                  "application/json; charset=utf-8")
            with _lock:
                entry = dict(_cache.get(window))
            payload = {
                "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(entry["fetched"])),
                "ageSeconds": round(time.time() - entry["fetched"]),
                "ttlSeconds": CACHE_TTL,
                "stale": bool(entry["stale"]),
                "staleReason": entry["error"],
                "windows": WINDOWS,
                **entry["data"],
            }
            return self._send(200, json.dumps(payload, ensure_ascii=False), "application/json; charset=utf-8")
        if path in ("/", "/index.html"):
            try:
                with open(UI_PATH, "rb") as f:
                    return self._send(200, f.read(), "text/html; charset=utf-8")
            except OSError as e:
                return self._send(500, f"找不到畫面檔案：{e}\n", "text/plain; charset=utf-8")
        return self._send(404, "not found\n", "text/plain; charset=utf-8")


def main():
    print(f"[start] OpenCost={OPENCOST_URL} 快取={CACHE_TTL}s 區間={WINDOWS} 監聽=:{LISTEN_PORT}", flush=True)
    threading.Thread(target=refresher, daemon=True).start()
    ThreadingHTTPServer(("", LISTEN_PORT), Handler).serve_forever()


if __name__ == "__main__":
    main()
