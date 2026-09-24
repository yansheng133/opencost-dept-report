#!/usr/bin/env python3
"""快照器的算術測試：python3 test_snapshot.py（不需要叢集，也不需要任何套件）

只測會直接影響帳單金額的事：單位換算、Pod 的實際宣告量、PVC 去重、
舊 schema 的相容性，以及區間查詢的邊界。
其他部分（API 呼叫、寫檔）壞掉會很吵；這幾個壞掉會無聲地算錯錢。
"""
import json
import os
import sys
import tempfile
from datetime import datetime, timezone

import snapshot as s

RESULTS = []


def eq(got, exp, what):
    ok = abs(got - exp) < 1e-9
    RESULTS.append(ok)
    print(f"{'PASS' if ok else 'FAIL'}  {what}\n      得到 {got}　預期 {exp}" if not ok
          else f"PASS  {what}")


def same(got, exp, what):
    """給不是數字的東西用（集合、字串、None）。"""
    ok = got == exp
    RESULTS.append(ok)
    print(f"{'PASS' if ok else 'FAIL'}  {what}\n      得到 {got!r}　預期 {exp!r}" if not ok
          else f"PASS  {what}")


def val(by_dept, dept, key):
    """取某部門的某個量，部門不存在就當 0。

    用 by_dept[dept] 直接取的話，算錯到「整個部門消失」的時候會噴 KeyError
    然後中斷測試，後面的檢查一項都跑不到——看到的是一個例外，不是一份失敗清單。
    """
    return by_dept.get(dept, {}).get(key, 0.0)


BASE = [{"resources": {"requests": {"cpu": "100m", "memory": "64Mi"}}},
        {"resources": {"requests": {"cpu": "200m", "memory": "64Mi"}}}]   # 合計 300m / 128Mi


def pod(inits=None):
    return {"containers": BASE, "initContainers": inits or []}


# ── 單位換算 ────────────────────────────────────────────────────────────
eq(s.cpu_cores("100m"), 0.1, "CPU 100m = 0.1 核")
eq(s.cpu_cores("1500m"), 1.5, "CPU 1500m = 1.5 核")
eq(s.cpu_cores("2"), 2.0, "CPU 2 = 2 核")
eq(s.mem_bytes("128Mi"), 134217728, "記憶體 128Mi 是 2 的冪")
eq(s.mem_bytes("1Gi"), 2**30, "記憶體 1Gi = 2^30")
eq(s.mem_bytes("500M"), 5e8, "記憶體 500M 是 10 的冪（跟 Mi 不同）")
# G / T 也要測：PVC 的 storage 常常寫成 10G 而不是 10Gi，而這兩個字尾原本
# 一個測試都沒有——把 G 的倍數改成 1e6 故意弄壞，整份測試竟然全過。
eq(s.mem_bytes("10G"), 1e10, "儲存 10G = 10^10（不是 Gi）")
eq(s.mem_bytes("2T"), 2e12, "儲存 2T = 2×10^12")
eq(s.mem_bytes("2Ti"), 2 * 2**40, "儲存 2Ti 是 2 的冪")
eq(s.mem_bytes("1024Ki"), 2**20, "儲存 1024Ki = 1Mi")

# ── Pod 的實際宣告量 ────────────────────────────────────────────────────
# 排程器取「一般容器總和」與「init container 峰值」的較大者
eq(s.effective_requests(pod([{"resources": {"requests": {"cpu": "500m"}}}]))[0], 0.5,
   "init 500m 大於一般總和 300m，取 init")
eq(s.effective_requests(pod([{"resources": {"requests": {"cpu": "50m"}}}]))[0], 0.3,
   "init 50m 小於一般總和 300m，取一般總和")
eq(s.effective_requests(pod())[1], 128 * 2**20, "記憶體取一般容器總和 128Mi")

# 原生 sidecar（restartPolicy=Always 的 init container）會跟一般容器並存
eq(s.effective_requests(pod([
    {"restartPolicy": "Always", "resources": {"requests": {"cpu": "400m"}}},
    {"resources": {"requests": {"cpu": "500m"}}}]))[0], 0.9,
   "sidecar 宣告在 init 前面：init 跑的時候它已啟動，峰值 500m+400m")
eq(s.effective_requests(pod([
    {"resources": {"requests": {"cpu": "500m"}}},
    {"restartPolicy": "Always", "resources": {"requests": {"cpu": "400m"}}}]))[0], 0.7,
   "sidecar 宣告在 init 後面：那時還沒起來，穩定狀態 300m+400m 勝出")

# 沒宣告 requests 的容器很常見（系統元件尤其多），不能炸也不能算成 None
eq(s.effective_requests({"containers": [{}]})[0], 0.0, "沒宣告 requests 就是 0")
eq(s.effective_requests({"containers": []})[1], 0.0, "沒有容器也不該出錯")

# ── Pod 掛了多少儲存 ────────────────────────────────────────────────────
PVC_BYTES = {("dept-it", "portal-data"): 2 * 2**30, ("dept-it", "logs"): 1 * 2**30}


def vol(claim):
    return {"persistentVolumeClaim": {"claimName": claim}}


eq(s.claimed_storage({"volumes": [vol("portal-data")]}, PVC_BYTES, "dept-it"), 2 * 2**30,
   "Pod 掛一個 2Gi 的 PVC")
eq(s.claimed_storage({"volumes": [vol("portal-data"), vol("logs")]}, PVC_BYTES, "dept-it"),
   3 * 2**30, "掛兩個不同 PVC 就加總")
# 同一個 PVC 掛成兩個 volume（不同 subPath）是合法的，但磁碟還是只有一份
eq(s.claimed_storage({"volumes": [vol("portal-data"), vol("portal-data")]}, PVC_BYTES, "dept-it"),
   2 * 2**30, "同一個 PVC 在同一個 Pod 掛兩次只算一次")
eq(s.claimed_storage({"volumes": [{"emptyDir": {}}]}, PVC_BYTES, "dept-it"), 0,
   "emptyDir 不是 PVC，不算儲存宣告")
eq(s.claimed_storage({}, PVC_BYTES, "dept-it"), 0, "沒有 volumes 也不該出錯")
# 同名 PVC 在別的 namespace 不該被誤抓——PVC 是 namespace 範圍的物件
eq(s.claimed_storage({"volumes": [vol("portal-data")]}, PVC_BYTES, "dept-rd"), 0,
   "PVC 名稱相同但 namespace 不同，不算進來")


# ── 分攤：儲存要按 PVC 算，不能按 Pod 加總 ──────────────────────────────
def snap(ts, pods, pvcs=None, schema=2):
    doc = {"schema": schema, "ts": ts, "interval_s": 3600, "pods": pods}
    if pvcs is not None:
        doc["pvcs"] = pvcs
    return doc


# 兩個 Pod 掛同一個 2 GiB 的 PVC：pod 的 pv 欄位各記 2 GiB（方便查誰掛了什麼），
# 但分攤只能算一份，否則同一顆磁碟會被收兩次錢。
SHARED = snap("2026-09-24T05:00:00Z",
              [{"ns": "dept-it", "pod": "a", "dept": "it", "env": "", "cpu": 0.1,
                "mem": 2**30, "pv": 2 * 2**30, "node": "n1", "owner": "Deployment/a"},
               {"ns": "dept-it", "pod": "b", "dept": "it", "env": "", "cpu": 0.1,
                "mem": 2**30, "pv": 2 * 2**30, "node": "n1", "owner": "Deployment/b"}],
              [{"ns": "dept-it", "name": "portal-data", "dept": "it", "env": "",
                "bytes": 2 * 2**30}])

by_dept, totals = s.allocate([SHARED], 3600)
eq(totals["pv_gib_h"], 2.0, "兩個 Pod 共用一個 2Gi PVC，分攤只算 2 GiB 小時（不是 4）")
eq(val(by_dept, "it", "pv_gib_h"), 2.0, "該部門的儲存也只記一份")
eq(val(by_dept, "it", "cpu_h"), 0.2, "CPU 是 Pod 的成本，兩個 Pod 要各算各的")
eq(val(by_dept, "it", "ram_gib_h"), 2.0, "記憶體同理")
eq(len(by_dept.get("it", {}).get("pods", ())), 2, "兩個不同的 owner 算兩個工作負載")

# PVC 的部門歸屬看 PVC 自己，不看掛它的 Pod——不然同一顆磁碟會有兩種歸屬
CROSS = snap("2026-09-24T05:00:00Z",
             [{"ns": "dept-it", "pod": "a", "dept": "rd", "env": "", "cpu": 0.0,
               "mem": 0, "pv": 2**30, "node": "n1", "owner": ""}],
             [{"ns": "dept-it", "name": "portal-data", "dept": "it", "env": "",
               "bytes": 2**30}])
by_dept, _ = s.allocate([CROSS], 3600)
eq(val(by_dept, "it", "pv_gib_h"), 1.0, "儲存記在 PVC 的部門（it）")
eq(val(by_dept, "rd", "pv_gib_h"), 0.0, "不記在掛它的 Pod 的部門（rd）")

# 沒有部門標籤的要落到 __unallocated__，不能被丟掉
NOLABEL = snap("2026-09-24T05:00:00Z",
               [{"ns": "x", "pod": "p", "dept": "", "env": "", "cpu": 1.0,
                 "mem": 0, "pv": 0, "node": "", "owner": ""}],
               [{"ns": "x", "name": "c", "dept": "", "env": "", "bytes": 2**30}])
by_dept, _ = s.allocate([NOLABEL], 3600)
eq(val(by_dept, "__unallocated__", "cpu_h"), 1.0, "沒有部門標籤的 Pod 歸到 __unallocated__")
eq(val(by_dept, "__unallocated__", "pv_gib_h"), 1.0, "沒有部門標籤的 PVC 也歸到 __unallocated__")

# ── schema 1 的舊快照不能讀壞 ───────────────────────────────────────────
# 舊快照會在 PVC 裡躺滿一個保留期，升級當天讀壞它們＝自己刪掉一個帳期的備援
OLD = snap("2026-09-24T05:00:00Z",
           [{"ns": "dept-it", "pod": "a", "dept": "it", "env": "", "cpu": 0.5,
             "mem": 2**30, "node": "n1", "owner": ""}], pvcs=None, schema=1)
by_dept, totals = s.allocate([OLD], 3600)
eq(val(by_dept, "it", "cpu_h"), 0.5, "schema 1 的 CPU 照樣算得出來")
eq(val(by_dept, "it", "pv_gib_h"), 0.0, "schema 1 沒有 pvcs，儲存當 0（不是爆掉）")
eq(totals["pv_gib_h"], 0.0, "schema 1 的儲存總計是 0")
# 新舊混在同一個區間是升級當下一定會遇到的情形
by_dept, totals = s.allocate([OLD, SHARED], 3600)
eq(totals["pv_gib_h"], 2.0, "新舊 schema 混在一起也算得出來（只有新的有儲存）")


# ── 區間查詢的邊界 ──────────────────────────────────────────────────────
def write(dirpath, doc):
    day = doc["ts"][:10]
    os.makedirs(os.path.join(dirpath, day), exist_ok=True)
    with open(os.path.join(dirpath, day, doc["ts"][11:19].replace(":", "") + ".json"), "w") as f:
        json.dump(doc, f)


def t(h, m):
    return datetime(2026, 9, 24, h, m, tzinfo=timezone.utc)


with tempfile.TemporaryDirectory() as tmp:
    body = [{"ns": "x", "pod": "p", "dept": "it", "env": "", "cpu": 1.0,
             "mem": 0, "pv": 0, "node": "", "owner": ""}]
    pvcs = [{"ns": "x", "name": "c", "dept": "it", "env": "", "bytes": 2**30}]
    for hh, mm in ((5, 0), (5, 30), (6, 0)):
        d = snap(f"2026-09-24T{hh:02d}:{mm:02d}:00Z", body, pvcs)
        d["interval_s"] = 1800
        write(tmp, d)
    # 兩種壞檔：寫到一半斷電的、以及沒有 ts 的。一個壞檔不該擋住整段區間，
    # 因為真的要用備援的時候，通常就是剛出過事、磁碟上有半個檔案的時候。
    with open(os.path.join(tmp, "2026-09-24", "051500.json"), "w") as f:
        f.write('{"schema":2,"pods":[]}')                  # 沒有 ts
    with open(os.path.join(tmp, "2026-09-24", "051600.json"), "w") as f:
        f.write("{壞掉的 JSON")

    # start 含、end 不含。邊界算錯會讓相鄰兩段區間重複收或漏收同一筆快照。
    same([d["ts"] for d in s.load_snapshots(tmp, t(5, 0), t(6, 0))],
         ["2026-09-24T05:00:00Z", "2026-09-24T05:30:00Z"], "start 含、end 不含")
    same([d["ts"] for d in s.load_snapshots(tmp, t(5, 1), t(6, 1))],
         ["2026-09-24T05:30:00Z", "2026-09-24T06:00:00Z"], "區間往後挪就換一組快照")

    res = s.build_allocation(tmp, t(5, 0), t(6, 0))
    eq(res["expected"], 2, "1 小時、間隔 1800s，期望 2 筆")
    eq(res["coverage"], 100.0, "找到 2 筆、期望 2 筆＝100%")
    eq(val(res["by_dept"], "it", "pv_gib_h"), 1.0, "2 筆 × 1 GiB × 0.5 小時 = 1 GiB 小時")

    # 快照比期望多（取樣點跟區間邊界沒對齊）時，完整度要封頂在 100。
    # 「完整度 200%」只會讓看的人不知道該信哪個數字。
    res = s.build_allocation(tmp, t(5, 0), t(5, 31))
    eq(res["expected"], 1, "31 分鐘、間隔 1800s，期望 1 筆")
    eq(len(res["snaps"]), 2, "實際找到 2 筆")
    eq(res["coverage"], 100.0, "找到的比期望多，完整度封頂 100")

    # 沒資料要回 None，不是回一份全零的結果——呼叫端得分得出「沒有」和「是零」
    same(s.build_allocation(tmp, t(9, 0), t(10, 0)), None, "區間內沒有快照就回 None")

print(f"\n{sum(RESULTS)}/{len(RESULTS)} 通過")
sys.exit(0 if all(RESULTS) else 1)
