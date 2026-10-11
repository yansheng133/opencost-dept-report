#!/usr/bin/env python3
"""儲存空間「申請 vs 實際」的測試：python3 test_storage.py（不需要叢集）

這一塊壞掉的方式都是無聲的：量少了不會報錯、沒量到會被當成 0、
一個 PVC 被算兩次也只是金額變大一點。所以每一種都要有一條測試。
"""
import os
import sys
import tempfile

import storage
import volume_usage as vu

R = []


def ok(cond, what, got=None):
    R.append(bool(cond))
    print(f"PASS  {what}" if cond else f"FAIL  {what}\n      得到 {got!r}")


GIB = 1024 ** 3
PV = "pvc-5926100a-5f60-41a7-be3d-92f851838a25"

# ── 目錄名稱 ────────────────────────────────────────────────
ok(vu.parse_dir(f"{PV}_dept-it_portal-data") == (PV, "dept-it", "portal-data"), "local-path 的目錄名稱拆得開")
ok(vu.parse_dir(f"{PV}_cost-report_cost-report-seals") == (PV, "cost-report", "cost-report-seals"),
   "namespace 與 PVC 名稱裡有連字號也拆得對")
ok(vu.parse_dir(f"{PV}_ns_data.v1") == (PV, "ns", "data.v1"), "PVC 名稱可以有點")
for bad in ("lost+found", f"{PV}_onlyns", "pvc-xyz_ns_name", f"{PV}_NS_name", f"{PV}_ns_name_extra"):
    ok(vu.parse_dir(bad) is None, f"不是 local-path 建的目錄不認：{bad[:40]}")

# ── du ─────────────────────────────────────────────────────
with tempfile.TemporaryDirectory() as root:
    d = os.path.join(root, f"{PV}_dept-it_portal-data")
    os.makedirs(os.path.join(d, "sub"))
    with open(os.path.join(d, "a.bin"), "wb") as f:
        f.write(os.urandom(256 * 1024))
    # 稀疏檔：長度 1 GiB，實際幾乎不佔空間
    with open(os.path.join(d, "sparse"), "wb") as f:
        f.truncate(GIB)
    os.link(os.path.join(d, "a.bin"), os.path.join(d, "sub", "hard"))
    os.symlink("/", os.path.join(d, "sub", "loop"))          # 跟著走的話會量整台機器
    used, files, errors = vu.disk_usage(d)
    ok(256 * 1024 <= used < 1024 * 1024, "算實際佔用的區塊：稀疏檔不算 1 GiB、硬連結只算一次", used)
    ok(errors == 0, "正常目錄沒有讀不到的項目", errors)

    os.makedirs(os.path.join(root, "not-a-pvc"))
    locked = os.path.join(d, "locked")
    os.makedirs(locked)
    open(os.path.join(locked, "x"), "w").close()
    os.chmod(locked, 0)
    try:
        r = vu.scan(root)
        v = r["volumes"][0] if r["volumes"] else {}
        if os.geteuid() != 0:   # root 讀得到 000 的目錄，這一條只在非 root 時有意義
            ok(v.get("errors", 0) >= 1, "讀不到的目錄要回報出來，不能無聲少算", v)
        ok(r["unparsed"] == 1 and len(r["volumes"]) == 1, "認不出來的目錄另外計數，不混進 PVC", r["unparsed"])
        text = vu.render(r, "node1")
        ok(f'cost_report_pvc_used_bytes{{pv="{PV}",pvc_namespace="dept-it",pvc="portal-data",node="node1"}}' in text,
           "metrics 的標籤是 pv / pvc_namespace / pvc（不用會跟抓取設定撞名的 namespace）")
        ok("cost_report_volume_scan_ok{node=\"node1\"} 1" in text, "根目錄讀得到時 scan_ok 是 1")
    finally:
        os.chmod(locked, 0o755)

r = vu.scan("/nonexistent-volume-root")
ok(r["volumes"] == [] and not r["rootOk"], "根目錄不存在（hostPath 掛錯）時 scan_ok 是 0，不假裝量過")
ok("cost_report_volume_scan_ok 0" in vu.render(None), "第一次掃描完成前 scan_ok 是 0")

# ── 併表 ───────────────────────────────────────────────────
key = f"cluster=rke2:name={PV}"
prom = "pvc-0a491df6-0dda-4b13-9e29-fbd2ccbcfa97"
by_cc = {
    "it": {"pvs": {key: {"byteHours": 47 * GIB, "cost": 0.047}}},
    # 實測：同一個 PV 也會以 0 元出現在 __unallocated__
    # 實測：0 元，但 byteHours 不是 0（幾秒鐘的量）
    "__unallocated__": {"pvs": {key: {"byteHours": 0.002 * GIB, "cost": 0.000002},
                                f"cluster=rke2:name={prom}": {"byteHours": 117.4 * GIB, "cost": 0.1174}}},
    "rd": {"pvs": None},
}
s = storage.build(
    by_cc,
    usage={PV: {"used": 900 * 1024, "peak": 1024 * 1024, "errors": 0},
           prom: {"used": 1.3 * GIB, "peak": 6 * GIB, "errors": 2}},
    capacity={PV: 2 * GIB, prom: 5 * GIB},
    claims={PV: ("dept-it", "portal-data"), prom: ("prometheus-system", "prometheus-server")})
rows = {r["pv"]: r for r in s["rows"]}
it = rows[PV]
ok(len(it["depts"]) == 1 and it["depts"][0]["id"] == "it", "0 元的重複歸屬不列出來，一個 PVC 不會看起來被兩個單位分攤", it["depts"])
ok(it["cost"] == 0.047 and abs(it["gibHours"] - 47.0) < 0.01, "金額與 GiB 時照 OpenCost", it)
ok(storage._depts({"__unallocated__": 0.0}) == [{"id": "__unallocated__", "cost": 0.0, "label": "未標籤"}],
   "只有一個歸屬、金額又是 0 時照樣列出來（不能變成沒人付）")
ok(it["usedPct"] == round(1024 * 1024 / (2 * GIB), 4), "使用率用區間最高點，不用最後一刻", it["usedPct"])
ok(abs(it["idleCost"] - 0.047 * (1 - 1 / 2048)) < 1e-4, "閒置金額 = 金額 × (1 − 最高用量 ÷ 容量)", it["idleCost"])
p = rows[prom]
ok(p["over"] and p["idleCost"] == 0, "用量超過容量（local-path 不擋）：標出來、閒置是 0，不是負的", p)
ok(p["partial"], "有讀不到的項目時標成「可能偏低」", p)
ok(p["depts"][0]["label"] == "未標籤", "__unallocated__ 在畫面上叫「未標籤」", p["depts"])
ok(s["rows"][0]["pv"] == prom, "依金額由大到小排")

s2 = storage.build(by_cc, usage={}, capacity={PV: 2 * GIB}, claims={PV: ("dept-it", "portal-data")})
ok(s2["idleCost"] is None and s2["measured"] == 0, "完全沒量到時閒置是「不知道」，不是 0", s2["idleCost"])
ok(all(r["usedPct"] is None for r in s2["rows"]) and [r for r in s2["rows"] if r["pv"] == PV][0]["capacityBytes"] == 2 * GIB,
   "沒量到的列照樣列出容量與金額，只是用量空著")
s3 = storage.build(by_cc, usage={PV: {"used": 0, "peak": 0}}, capacity={PV: 2 * GIB})
ok(rows and s3["rows"][[r["pv"] for r in s3["rows"]].index(PV)]["usedPct"] == 0,
   "量到 0 是 0（空的 PVC），跟沒量到不一樣")
ok(storage.build(by_cc, usage={PV: {"peak": 1}})["rows"][1]["usedPct"] is None,
   "不知道容量就不算使用率")
s4 = storage.build(by_cc, usage={PV: {"ns": "dept-it", "pvc": "portal-data", "peak": 1}})
ok([r for r in s4["rows"] if r["pv"] == PV][0]["pvc"] == "portal-data",
   "kube-state-metrics 沒資料時，PVC 名稱退回用量測元件從目錄名稱拆出來的")
ok(storage.pv_from_key("cluster=rke2:name=pvc-1") == "pvc-1" and storage.pv_from_key("pvc-2") == "pvc-2",
   "OpenCost 的 pvs 鍵拆得出 PV 名稱")
ok(storage.build({}) == {"rows": [], "total": 0, "idleCost": None, "measuredCost": 0, "measured": 0, "count": 0},
   "沒有任何 PVC 時不會炸")

print(f"\n{sum(R)}/{len(R)} 通過")
sys.exit(0 if all(R) else 1)
