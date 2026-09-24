#!/usr/bin/env python3
"""快照器的算術測試：python3 test_snapshot.py（不需要叢集，也不需要任何套件）

只測兩件會直接影響帳單金額的事：單位換算，以及 Pod 的實際宣告量怎麼算。
其他部分（API 呼叫、寫檔）壞掉會很吵；這兩個壞掉會無聲地算錯錢。
"""
import sys

import snapshot as s

RESULTS = []


def eq(got, exp, what):
    ok = abs(got - exp) < 1e-9
    RESULTS.append(ok)
    print(f"{'PASS' if ok else 'FAIL'}  {what}\n      得到 {got}　預期 {exp}" if not ok
          else f"PASS  {what}")


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

print(f"\n{sum(RESULTS)}/{len(RESULTS)} 通過")
sys.exit(0 if all(RESULTS) else 1)
