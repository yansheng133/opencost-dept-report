#!/usr/bin/env python3
"""從宣告量快照算出分攤依據，並且可以跟 OpenCost 的實測值對照。

兩個用途：
  1. Prometheus 有斷層時，用這個當第二份分攤依據（降級階梯的第 3 階）。
  2. 在資料正常的期間跑 --compare，量出「宣告量法」到底差實測多少。
     沒有量過誤差的備援方案，跟沒有備援方案差不多——真的要用的時候，
     你不知道該不該相信它算出來的數字。

用法：
  python3 fallback.py --snapshots ./snapshots --start 2026-09-24T05:00:00Z --end 2026-09-24T05:30:00Z
  python3 fallback.py --snapshots ./snapshots --start ... --end ... --compare http://127.0.0.1:9003
  python3 fallback.py --snapshots ./snapshots --last 30m --compare http://127.0.0.1:9003

  --cpu-rate / --ram-rate / --pv-rate 給了就換算成金額
  （單位：每 vCPU 小時、每 GiB 小時、每儲存 GiB 小時）。

儲存的分攤依據是快照裡的 `pvcs`（每個 PVC 一列），不是 Pod 的 `pv` 欄位：
多個 Pod 掛同一個 PVC 時，用 Pod 加總會重複計算，而磁碟只有一份。
schema 1 的舊快照沒有 `pvcs`，儲存一律算 0——當時本來就沒收這筆資料。
"""
import argparse
import json
import re
import sys
import urllib.request
from datetime import datetime, timedelta, timezone

# 讀快照、算分攤的那一段跟 snapshot.py 的查詢介面共用同一份實作。
# 抄一份過來比較省事，但兩份實作遲早會漂移，而漂移的症狀是「離線算出來的帳」
# 跟「cost-report 線上問到的帳」對不起來，還很難查是哪一邊錯。
from snapshot import GIB, build_allocation, departments_json, parse_ts


def parse_dur(s):
    m = re.fullmatch(r"(\d+)([mhd])", s)
    if not m:
        sys.exit(f"看不懂的長度：{s}（要像 30m、2h、1d）")
    n, unit = int(m.group(1)), m.group(2)
    return timedelta(minutes=n) if unit == "m" else timedelta(hours=n) if unit == "h" else timedelta(days=n)


def opencost_measured(url, start, end):
    """跟 OpenCost 要同一段絕對區間的實測分攤，用來當對照組。"""
    window = f"{start.strftime('%Y-%m-%dT%H:%M:%SZ')},{end.strftime('%Y-%m-%dT%H:%M:%SZ')}"
    q = (f"{url.rstrip('/')}/allocation?window={window}"
         f"&aggregate=label:cost-center&accumulate=true&includeIdle=false")
    with urllib.request.urlopen(q, timeout=30) as r:
        data = json.load(r).get("data")
    if not data:
        return {}, 0.0
    sets = data if isinstance(data, list) else [data]
    out, minutes = {}, 0.0
    for s in sets:
        for name, a in (s or {}).items():
            if name in ("__idle__",):
                continue
            dept = name if name != "__unallocated__" else "__unallocated__"
            row = out.setdefault(dept, {"cpu_h": 0.0, "ram_gib_h": 0.0, "pv_gib_h": 0.0})
            row["cpu_h"] += a.get("cpuCoreHours", 0.0)
            row["ram_gib_h"] += a.get("ramByteHours", 0.0) / GIB
            # pvByteHours 是 OpenCost 對儲存的實測；沒有掛 PVC 的分攤不會有這個欄位
            row["pv_gib_h"] += (a.get("pvByteHours") or 0.0) / GIB
            # OpenCost 實際涵蓋的時間常常比你要求的短，兩邊時長不同就不能直接比總量
            minutes = max(minutes, float(a.get("minutes") or 0))
    return out, minutes


def share(rows, key):
    total = sum(r[key] for r in rows.values()) or 1.0
    return {d: r[key] / total * 100 for d, r in rows.items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--snapshots", required=True)
    ap.add_argument("--start")
    ap.add_argument("--end")
    ap.add_argument("--last", help="用「現在往前推」取代 --start/--end，例如 30m")
    ap.add_argument("--compare", help="OpenCost API 位址，給了就跟實測值對照")
    ap.add_argument("--cpu-rate", type=float, default=None)
    ap.add_argument("--ram-rate", type=float, default=None)
    # 沒給就當 0。給了 cpu/ram 單價卻沒給儲存單價時，金額欄會少掉儲存那一塊，
    # 而少掉的部分在報表上看不出來——所以下面會在表頭註明它有沒有含儲存。
    ap.add_argument("--pv-rate", type=float, default=None)
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()

    if a.last:
        end = datetime.now(timezone.utc).replace(microsecond=0)
        start = end - parse_dur(a.last)
    elif a.start and a.end:
        start, end = parse_ts(a.start), parse_ts(a.end)
    else:
        sys.exit("要給 --start/--end，或 --last")

    res = build_allocation(a.snapshots, start, end)
    if res is None:
        sys.exit(f"這段區間沒有任何快照：{start:%Y-%m-%dT%H:%M:%SZ} ~ {end:%Y-%m-%dT%H:%M:%SZ}")
    snaps, interval_s = res["snaps"], res["interval_s"]
    expected, coverage = res["expected"], res["coverage"]
    by_dept = res["by_dept"]
    total_cpu, total_ram = res["totals"]["cpu_h"], res["totals"]["ram_gib_h"]
    total_pv = res["totals"]["pv_gib_h"]
    measured, measured_min = opencost_measured(a.compare, start, end) if a.compare else ({}, 0.0)

    if a.json:
        out = {
            "window": {"start": f"{start:%Y-%m-%dT%H:%M:%SZ}", "end": f"{end:%Y-%m-%dT%H:%M:%SZ}"},
            "basis": "declared",
            "snapshot_coverage_pct": round(coverage, 2),
            "samples": {"found": len(snaps), "expected": expected, "interval_s": interval_s},
            "departments": departments_json(by_dept),
        }
        print(json.dumps(out, ensure_ascii=False, indent=2))
        return

    print(f"區間　　{start:%Y-%m-%dT%H:%M:%SZ} ~ {end:%Y-%m-%dT%H:%M:%SZ}"
          f"（{(end - start).total_seconds() / 3600:.2f} 小時）")
    print(f"快照　　{len(snaps)}/{expected} 筆，間隔 {interval_s}s，完整度 {coverage:.1f}%"
          + ("" if coverage >= 99 else "　← 備援資料本身也有缺漏，結果只能當參考"))
    print(f"依據　　宣告量（declared）：只看 requests，不需要 Prometheus\n")

    d_share_cpu = share(by_dept, "cpu_h")
    m_share_cpu = share(measured, "cpu_h") if measured else {}

    head = (f"{'部門':<16}{'宣告核心小時':>14}{'宣告GiB小時':>14}"
            f"{'宣告GiB小時(儲存)':>18}{'份額%':>9}")
    if measured:
        head += f"{'實測份額%':>11}{'差距pp':>9}"
    if a.cpu_rate is not None and a.ram_rate is not None:
        head += f"{'金額(含儲存)' if a.pv_rate is not None else '金額(不含儲存)':>12}"
    print(head)
    print("-" * 94)

    worst = 0.0
    for dept in sorted(by_dept, key=lambda d: -by_dept[d]["cpu_h"]):
        r = by_dept[dept]
        line = (f"{dept:<16}{r['cpu_h']:>14.4f}{r['ram_gib_h']:>14.4f}"
                f"{r['pv_gib_h']:>18.4f}{d_share_cpu[dept]:>9.2f}")
        if measured:
            ms = m_share_cpu.get(dept)
            if ms is None:
                line += f"{'—':>11}{'—':>9}"
            else:
                delta = d_share_cpu[dept] - ms
                worst = max(worst, abs(delta))
                line += f"{ms:>11.2f}{delta:>+9.2f}"
        if a.cpu_rate is not None and a.ram_rate is not None:
            amount = r["cpu_h"] * a.cpu_rate + r["ram_gib_h"] * a.ram_rate
            amount += r["pv_gib_h"] * (a.pv_rate or 0.0)
            line += f"{amount:>12.4f}"
        print(line)

    print(f"\n合計　　{total_cpu:.4f} 核心小時 / {total_ram:.4f} GiB 小時 / "
          f"{total_pv:.4f} GiB 小時（儲存）")
    if measured:
        m_cpu = sum(r["cpu_h"] for r in measured.values())
        m_pv = sum(r["pv_gib_h"] for r in measured.values())
        # 兩邊各自除以自己實際涵蓋的時長，才是同一個基準
        d_hours = len(snaps) * interval_s / 3600.0
        m_hours = measured_min / 60.0 or d_hours
        d_rate, m_rate = total_cpu / d_hours, m_cpu / m_hours
        gap = (d_rate - m_rate) / m_rate * 100 if m_rate else 0.0
        print(f"實測　　{m_cpu:.4f} 核心小時（涵蓋 {measured_min:.1f} 分鐘）")
        print(f"換算成速率　宣告 {d_rate:.4f} 核 vs 實測 {m_rate:.4f} 核（宣告量法差 {gap:+.2f}%）")
        # 儲存的誤差方向跟 CPU/記憶體不同：PVC 是「宣告多少就佔多少磁碟」，
        # 兩邊應該幾乎一樣。差很多通常不是方法誤差，是有 PVC 沒被算到
        # （例如 OpenCost 抓不到、或快照的 RBAC 少了 persistentvolumeclaims）。
        d_pv_rate, m_pv_rate = total_pv / d_hours, m_pv / m_hours
        pv_gap = (d_pv_rate - m_pv_rate) / m_pv_rate * 100 if m_pv_rate else 0.0
        print(f"儲存　　宣告 {d_pv_rate:.4f} GiB vs 實測 {m_pv_rate:.4f} GiB"
              + (f"（差 {pv_gap:+.2f}%）" if m_pv_rate else "（實測沒有 pvByteHours，無法對照）"))
        print(f"\n份額最大差距：{worst:.2f} 個百分點")
        print("宣告量法會低估「用量超過申請量」的工作負載——OpenCost 計價依 max(申請, 用量)，"
              "\n而快照只看得到申請量。差距多大取決於你的叢集有多少爆量的工作負載。")


if __name__ == "__main__":
    main()
