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

import billing

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
# 價目表：單價是政策，不該寫死在程式裡，也不該只存在 OpenCost 的 values 裡面
RATECARD_PATH = os.environ.get("RATECARD_PATH", "/config/ratecard.json")
# 快照器：資料有斷層時，跟它要「宣告量」當第二份分攤依據。留空就不啟用。
SNAPSHOTTER_URL = os.environ.get(
    "SNAPSHOTTER_URL", "http://cost-snapshotter.cost-report.svc.cluster.local:80").rstrip("/")
# 帳期封存：每天結帳一次並凍結。沒有這個，上個月的帳單過了 retention 就查不到了。
SEAL_DIR = os.environ.get("SEAL_DIR", "/seals")
SEAL_ENABLED = os.environ.get("SEAL_ENABLED", "1") != "0"
SEAL_WINDOW = os.environ.get("SEAL_WINDOW", "24h")   # 封存用的區間長度（整天）
# 使用率折線圖的取樣間隔。點太密會看不出趨勢，太疏會把日夜曲線抹平。
STEPS = dict(kv.split("=", 1) for kv in
             os.environ.get("SERIES_STEPS", "1h=5m,24h=1h,7d=6h").split(",") if "=" in kv)

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
    容器是 UTC、你的筆電是 UTC+8，差 8 小時的斷層報告完全看不出來是時區問題。

    查詢的那一天完全沒有資料時，OpenCost 回的 start 會是 None。這裡刻意丟 ValueError
    而不是讓 AttributeError 冒出去：呼叫端接的是 ValueError，讓不同的壞法走同一條路，
    才不會有某個路徑漏接而整條執行緒死掉（封存執行緒就這樣掛過一次）。
    """
    if not s:
        raise ValueError("沒有時間字串（這段區間可能完全沒有資料）")
    txt = s.strip().replace("+0000", "Z").replace("+00:00", "Z")
    if "." in txt:
        txt = txt.split(".")[0] + "Z"
    return calendar.timegm(time.strptime(txt, "%Y-%m-%dT%H:%M:%SZ"))


def assets_total(window):
    """資產（節點、磁碟）的實際成本，用來跟分攤總額對帳。

    **對帳一定要用「含閒置」的分攤總額。** 不含閒置的話會看起來短少兩成以上
    （實測 −26%），然後你會花一整個下午找一個根本不存在的漏洞：
    閒置就是節點買了沒人用的那部分，它當然算在資產成本裡。
    """
    try:
        params = {"window": window, "aggregate": "type", "accumulate": "true"}
        url = f"{OPENCOST_URL}/assets?{urllib.parse.urlencode(params)}"
        with urllib.request.urlopen(url, timeout=HTTP_TIMEOUT) as r:
            body = json.load(r)
        data = body.get("data")
        items = data if isinstance(data, dict) else (data[0] if data else {})
        by_type = {}
        for v in (items or {}).values():
            t = v.get("type") or "其他"
            by_type[t] = _round(by_type.get(t, 0) + float(v.get("totalCost") or 0))
        return _round(sum(by_type.values())), by_type
    except (urllib.error.URLError, OSError, ValueError, KeyError, TypeError) as e:
        print(f"[warn] 讀不到資產成本：{e}", flush=True)
        return None, {}


def target_health(start_iso, end_iso):
    """有多少監控目標在這段期間掉線過。

    **部分缺漏比全部缺漏危險。** 全掛你看得出來；少數節點的 exporter 掉了，
    報表照樣產出、看起來一切正常，只是那幾台的成本悄悄少算。
    所以要比的不是「有沒有資料」，是「預期的 target 數 vs 實際回報的數」。
    """
    if not PROM_URL:
        return None
    try:
        start, end = _parse_iso(start_iso), _parse_iso(end_iso)
    except (TypeError, ValueError):
        return None
    span = end - start
    if span <= 0:
        return None
    step = max(60.0, span / 500)
    try:
        total = _prom("/api/v1/query_range",
                      {"query": "count by (job) (up)", "start": start, "end": end, "step": int(step)})
        healthy = _prom("/api/v1/query_range",
                        {"query": "sum by (job) (up)", "start": start, "end": end, "step": int(step)})
    except (urllib.error.URLError, OSError, RuntimeError, ValueError):
        return None

    def series(data):
        out = {}
        for s_ in data.get("result") or []:
            job = (s_.get("metric") or {}).get("job") or "?"
            out[job] = [float(v) for _, v in s_.get("values", [])]
        return out

    tot, hea = series(total), series(healthy)
    rows = []
    for job, values in sorted(tot.items()):
        if not values:
            continue
        expected = max(values)
        h = hea.get(job) or []
        worst = min(h) if h else 0.0
        # 掉線的樣本數 ÷ 總樣本數：偶爾一次重啟跟長期少一台，意義完全不同
        degraded = sum(1 for i, v in enumerate(values)
                       if (h[i] if i < len(h) else 0) < v)
        rows.append({
            "job": job,
            "expected": int(expected),
            "worstHealthy": int(worst),
            "degradedPct": _round(degraded / len(values) * 100, 2),
            "status": "ok" if worst >= expected and degraded == 0 else "degraded",
        })
    return rows


def declared_fill(gaps, rates):
    """資料有斷層時，跟快照器要「宣告量」，把那幾段的成本補回來。

    補的金額 = 宣告的資源小時數 × 價目表單價。刻意不用「前後時段的平均」，
    因為宣告量是那段時間**實際存在的事實**（從 API server 拿的），
    而平均是推測。兩者都會標成推估，但前者說得出依據。

    回傳每個部門被補了多少，以及補值本身的可信度（快照自己完不完整）。
    """
    if not SNAPSHOTTER_URL or not gaps or not rates:
        return None
    filled, notes, covered = {}, [], []
    for g in gaps:
        try:
            q = urllib.parse.urlencode({"start": g["start"], "end": g["end"]})
            with urllib.request.urlopen(f"{SNAPSHOTTER_URL}/allocation?{q}", timeout=PROM_TIMEOUT) as r:
                doc = json.load(r)
        except urllib.error.HTTPError as e:
            notes.append(f"{g['start']}～{g['end']}：快照器沒有這段的資料（HTTP {e.code}）")
            continue
        except (urllib.error.URLError, OSError, ValueError) as e:
            notes.append(f"{g['start']}～{g['end']}：問不到快照器（{e}）")
            continue
        covered.append(doc.get("snapshotCoveragePct"))
        for dept, v in (doc.get("departments") or {}).items():
            cost = (float(v.get("cpuCoreHours") or 0) * (rates.get("cpu") or 0)
                    + float(v.get("ramGiBHours") or 0) * (rates.get("ram") or 0)
                    + float(v.get("pvGiBHours") or 0) * (rates.get("storage") or 0))
            filled[dept] = _round(filled.get(dept, 0) + cost)
    if not filled and not notes:
        return None
    return {
        "byDept": filled,
        "total": _round(sum(filled.values())),
        "basis": "declared",
        # 用一份殘缺的備援去補另一份殘缺的資料，錯得更難發現，所以這個數字要顯示出來
        "snapshotCoveragePct": _round(min(covered), 2) if covered else None,
        "notes": notes,
    }


def usage_series(window):
    """各部門的使用率隨時間變化。

    使用率 = 實際用量 ÷ 申請量。這跟帳單金額是兩回事：金額看的是「付了多少」，
    使用率看的是「申請的東西有沒有在用」。一個部門可以金額很低但使用率也很低——
    那代表它申請的不多，但申請的那點也沒在用。

    **沒有資料的區段要讓折線斷開，不可以內插。** 把兩個相隔三天的點連起來，
    中間那條線看起來跟真的量到一樣，而那三天可能是監控掛掉。
    """
    step = STEPS.get(window)
    if not step:
        return None
    try:
        params = {"window": window, "aggregate": "label:cost-center",
                  "accumulate": "false", "step": step}
        url = f"{OPENCOST_URL}/allocation/compute?{urllib.parse.urlencode(params)}"
        req = urllib.request.Request(url, headers={"Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=HTTP_TIMEOUT) as r:
            body = json.load(r)
        segments = body.get("data")
        if not isinstance(segments, list):
            return None
    except (urllib.error.URLError, OSError, ValueError, KeyError) as e:
        print(f"[warn] 讀不到使用率序列：{e}", flush=True)
        return None

    step_min = {"m": 1, "h": 60, "d": 1440}.get(step[-1], 60) * int(step[:-1] or 1)

    # 空的區段沒有 start 欄位，但每一段的長度是固定的，所以可以從
    # 「第一個有資料的區段」往前後推算出來。有時間戳才放得上時間軸。
    base_idx, base_ts = None, None
    for i, seg in enumerate(segments):
        row = next((v for k, v in (seg or {}).items() if v and not k.startswith("__")), None)
        if row and row.get("start"):
            try:
                base_idx, base_ts = i, _parse_iso(row["start"])
                break
            except ValueError:
                continue
    if base_ts is None:
        return None

    def seg_start(i):
        return time.strftime("%Y-%m-%dT%H:%M:%SZ",
                             time.gmtime(base_ts + (i - base_idx) * step_min * 60))

    points, covered = [], 0
    for idx, seg in enumerate(segments):
        rows = {k: v for k, v in (seg or {}).items() if v and not k.startswith("__")}
        any_row = next(iter(rows.values()), {}) if rows else {}
        minutes = float(any_row.get("minutes") or 0)
        # 沒有資料的區段**也要放進序列**，只是 depts 給 None。
        # 把它們濾掉的話，前端會把剩下的點均勻攤開——於是 6 小時的間隔
        # 跟 3 天的間隔在圖上長得一模一樣，X 軸就開始說謊了。
        # 區間邊緣常有只涵蓋幾分鐘的碎片，當成完整一格會變成不存在的尖點，一樣當作沒有。
        if not rows or minutes < step_min * 0.25:
            points.append({"start": seg_start(idx), "depts": None})
            continue
        covered += 1
        depts = {}
        for dept, v in rows.items():
            req_cpu = float(v.get("cpuCoreRequestAverage") or 0)
            req_ram = float(v.get("ramByteRequestAverage") or 0)
            depts[dept] = {
                # 沒有申請量就沒有「使用率」這個概念，回 None 讓前端斷線
                "cpu": _round(v.get("cpuEfficiency"), 4) if req_cpu > 0 else None,
                "ram": _round(v.get("ramEfficiency"), 4) if req_ram > 0 else None,
                "cpuReq": _round(req_cpu, 3),
                "cpuUse": _round(v.get("cpuCoreUsageAverage"), 3),
            }
        points.append({"start": any_row.get("start"), "end": any_row.get("end"),
                       "minutes": _round(minutes, 1), "depts": depts})
    if not covered:
        return None
    return {"step": step, "stepMinutes": step_min, "points": points,
            "covered": covered, "expected": len(segments)}


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
    # 區分「系統元件」與「沒標籤的應用」很重要：前者不該標成某個部門——它是平台共用成本，
    # 要的是分攤規則（按比例攤回、或平台吸收）；後者才是標籤治理該去補的對象。
    # 把兩者混在一起，就會產生「把 kube-system 標給製造部」這種荒謬的建議。
    unalloc_rows = sorted(
        ({"ns": k, "total": _round(v.get("totalCost")),
          "system": any(k == pfx or k.startswith(pfx) for pfx in SYSTEM_NS)}
         for k, v in by_ns.items() if not k.startswith("__") and k not in dept_ns),
        key=lambda r: -r["total"])

    # 單價由資料反推：金額 ÷ 用量。避免在兩個地方各寫一次單價。
    # 只能用有標籤的部門來反推：__idle__ 有金額但用量欄位是 0，混進來會把單價灌高。
    def implied(cost_key, usage_key, div=1.0):
        rows = [v for k, v in by_cc.items() if not k.startswith("__")]
        cost = sum(float(r.get(cost_key) or 0) for r in rows)
        usage = sum(float(r.get(usage_key) or 0) for r in rows) / div
        return _round(cost / usage, 6) if usage else None

    any_dept = next((v for k, v in by_cc.items() if not k.startswith("__")), {})
    measured_total = _round(sum(float(v.get("totalCost") or 0) for v in by_cc.values()))
    w_start, w_end = any_dept.get("start"), any_dept.get("end")

    implied_rates = {
        "cpu": implied("cpuCost", "cpuCoreHours"),
        "ram": implied("ramCost", "ramByteHours", GIB),
        "storage": implied("pvCost", "pvByteHours", GIB),
    }
    card = billing.load_ratecard(RATECARD_PATH)
    version = billing.rate_at(card, w_start)
    # 補值要用「當期生效的單價」；沒有價目表就退回用反推的單價，並在畫面上講明白
    rates = {k: (version or {}).get(k, implied_rates.get(k)) for k in implied_rates} if version \
        else dict(implied_rates)

    cov = coverage(w_start, w_end)
    gaps = (cov or {}).get("gaps") or []
    fill = declared_fill(gaps, rates) if gaps else None

    # 把補值加進各部門金額，並記下每一列有多少是推估的
    for d in depts:
        est = _round((fill or {}).get("byDept", {}).get(d["id"], 0))
        d["measured"] = d["total"]
        d["estimated"] = est
        d["total"] = _round(d["total"] + est)
        d["shared"] = _round(d["shared"] + est)
        d["basis"] = "measured" if est <= 0 else "mixed"
    est_unalloc = _round((fill or {}).get("byDept", {}).get("__unallocated__", 0))
    total = _round(measured_total + ((fill or {}).get("total") or 0))
    estimated_total = _round((fill or {}).get("total") or 0)
    estimated_pct = _round(estimated_total / total * 100, 2) if total else 0.0

    assets, assets_by_type = assets_total(window)
    recon = billing.reconcile(total, assets)
    # 把「每項資源佔總成本多少」一起帶進去：金額太小的項目，反推單價本來就不準，
    # 不該讓它擋住整張帳單（實測兩分鐘的區間裡儲存只花 0.0001，誤差 5.6%）。
    cost_by_res = {}
    for key, col in (("cpu", "cpuCost"), ("ram", "ramCost"), ("storage", "pvCost")):
        cost_by_res[key] = sum(float(v.get(col) or 0) for k, v in by_cc.items()
                               if not k.startswith("__"))
    res_sum = sum(cost_by_res.values())
    share = {k: (v / res_sum if res_sum else None) for k, v in cost_by_res.items()}
    rate_status = billing.rate_check(version, implied_rates, share) if version else {}
    unalloc_pct = _round((unalloc_sep + est_unalloc) / total * 100, 2) if total else 0.0
    gate = billing.billing_gate((cov or {}).get("pct"), estimated_pct, recon,
                                rate_status, unalloc_pct, has_data=total > 0)

    return {
        "window": {
            "query": window,
            "start": w_start, "end": w_end,
            "minutes": _round(any_dept.get("minutes"), 1),
        },
        "cluster": CLUSTER_LABEL,
        "coverage": cov,
        "targets": target_health(w_start, w_end),
        "series": usage_series(window),
        "rates": rates,
        "ratesImplied": implied_rates,
        "rateCard": {
            "loaded": bool(card),
            "currency": (card or {}).get("currency"),
            "note": (card or {}).get("note"),
            "version": version,
            "versions": (card or {}).get("versions") or [],
            "check": rate_status,
        },
        "depts": depts, "idle": idle,
        "unallocated": {"separate": _round(unalloc_sep + est_unalloc), "shared": unalloc_shr},
        "total": total,
        "measuredTotal": measured_total,
        "basis": {
            "estimatedTotal": estimated_total,
            "estimatedPct": estimated_pct,
            "fill": fill,
            "labels": billing.BASIS_LABELS,
        },
        "reconciliation": dict(recon, byType=assets_by_type),
        "gate": gate,
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


def seal_completed_days(days_back=7):
    """把已經結束的每一天封存起來。

    為什麼需要：Prometheus 的 retention 是有限的（這個環境是 7 天），
    所以「上個月的帳單」在時間過去之後根本查不出來，而且每次重查的結果
    還可能因為資料過期而不同。封存之後，同一天不管什麼時候查都是同一份數字。

    只封存**已經結束**的日子——今天還沒過完，封了也是半張帳單。
    """
    today = time.strftime("%Y-%m-%d", time.gmtime())
    results = []
    for i in range(1, days_back + 1):
        day = time.strftime("%Y-%m-%d", time.gmtime(time.time() - i * 86400))
        if day >= today or billing.read_seal(SEAL_DIR, day):
            continue
        try:
            data = build_report(f"{day}T00:00:00Z,{day}T23:59:59Z")
        except (urllib.error.URLError, OSError, RuntimeError, ValueError, AttributeError) as e:
            print(f"[warn] 封存 {day} 失敗：{type(e).__name__}: {e}", flush=True)
            continue
        if not data["window"].get("start") or not data["total"]:
            # 那一天根本沒有資料（早於開始收集的時間，或叢集是關的）。
            # 封一張零元的帳單比不封更糟：它看起來像「那天真的沒花錢」。
            print(f"[seal] {day} 沒有資料，跳過", flush=True)
            continue
        payload = {
            "window": data["window"],
            "cluster": data["cluster"],
            "totals": {"total": data["total"], "measured": data["measuredTotal"],
                       "idle": data["idle"], "unallocated": data["unallocated"]["separate"]},
            "depts": [{"id": d["id"], "total": d["total"], "measured": d["measured"],
                       "estimated": d["estimated"], "basis": d["basis"]} for d in data["depts"]],
            "coverage": {"pct": (data.get("coverage") or {}).get("pct"),
                         "gapMinutes": (data.get("coverage") or {}).get("gapMinutes")},
            "basis": {"estimatedTotal": data["basis"]["estimatedTotal"],
                      "estimatedPct": data["basis"]["estimatedPct"]},
            "rates": data["rates"],
            "rateVersion": (data["rateCard"] or {}).get("version"),
            "reconciliation": {k: v for k, v in data["reconciliation"].items() if k != "byType"},
            "gate": data["gate"]["verdict"],
        }
        res = billing.seal_day(SEAL_DIR, day, payload)
        print(f"[seal] {day} → {res['status']}", flush=True)
        results.append(dict(res, day=day))
    return results


def sealer():
    while True:
        if SEAL_ENABLED:
            try:
                seal_completed_days()
            except Exception as e:                  # 封存失敗不可以拖垮報表服務
                print(f"[warn] 封存執行緒錯誤：{type(e).__name__}: {e}", flush=True)
        time.sleep(3600)


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
        if path == "/api/seals":
            # 已封存的帳期。intact 是重算雜湊的結果——只存雜湊不驗算等於沒有防竄改。
            return self._send(200, json.dumps({"dir": SEAL_DIR, "enabled": SEAL_ENABLED,
                                               "seals": billing.list_seals(SEAL_DIR)},
                                              ensure_ascii=False),
                              "application/json; charset=utf-8")
        if path == "/api/seal":
            day = (qs.get("day") or [""])[0]
            doc = billing.read_seal(SEAL_DIR, day) if day else None
            if not doc:
                return self._send(404, json.dumps({"error": f"沒有 {day} 的封存"}, ensure_ascii=False),
                                  "application/json; charset=utf-8")
            return self._send(200, json.dumps(doc, ensure_ascii=False),
                              "application/json; charset=utf-8")
        if path in ("/", "/index.html"):
            try:
                with open(UI_PATH, "rb") as f:
                    return self._send(200, f.read(), "text/html; charset=utf-8")
            except OSError as e:
                return self._send(500, f"找不到畫面檔案：{e}\n", "text/plain; charset=utf-8")
        return self._send(404, "not found\n", "text/plain; charset=utf-8")


def main():
    print(f"[start] OpenCost={OPENCOST_URL} 快取={CACHE_TTL}s 區間={WINDOWS} 監聽=:{LISTEN_PORT}", flush=True)
    card = billing.load_ratecard(RATECARD_PATH)
    print(f"[start] 價目表 {RATECARD_PATH}：{'已載入 %d 個版本' % len(card['versions']) if card else '沒有（單價改用反推）'}"
          f"　封存 {'開啟 → ' + SEAL_DIR if SEAL_ENABLED else '關閉'}", flush=True)
    threading.Thread(target=refresher, daemon=True).start()
    if SEAL_ENABLED:
        threading.Thread(target=sealer, daemon=True).start()
    ThreadingHTTPServer(("", LISTEN_PORT), Handler).serve_forever()


if __name__ == "__main__":
    main()
