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

  --cpu-rate / --ram-rate 給了就換算成金額（單位：每 vCPU 小時、每 GiB 小時）。
"""
import argparse
import json
import os
import re
import sys
import urllib.request
from datetime import datetime, timedelta, timezone

GIB = 2**30


def parse_ts(s):
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)


def parse_dur(s):
    m = re.fullmatch(r"(\d+)([mhd])", s)
    if not m:
        sys.exit(f"看不懂的長度：{s}（要像 30m、2h、1d）")
    n, unit = int(m.group(1)), m.group(2)
    return timedelta(minutes=n) if unit == "m" else timedelta(hours=n) if unit == "h" else timedelta(days=n)


def load(snap_dir, start, end):
    """讀出區間內的快照。回傳 (快照清單, 取樣間隔秒數)。"""
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
    """每一張快照代表 interval 這段時間的狀態，乘起來就是宣告的資源小時數。"""
    hours = interval_s / 3600.0
    by_dept, total_cpu, total_ram = {}, 0.0, 0.0
    for snap in snaps:
        for p in snap["pods"]:
            dept = p["dept"] or "__unallocated__"
            row = by_dept.setdefault(dept, {"cpu_h": 0.0, "ram_gib_h": 0.0, "pods": set()})
            row["cpu_h"] += p["cpu"] * hours
            row["ram_gib_h"] += p["mem"] / GIB * hours
            row["pods"].add(f"{p['ns']}/{p['owner'] or p['pod']}")
            total_cpu += p["cpu"] * hours
            total_ram += p["mem"] / GIB * hours
    return by_dept, total_cpu, total_ram


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
            row = out.setdefault(dept, {"cpu_h": 0.0, "ram_gib_h": 0.0})
            row["cpu_h"] += a.get("cpuCoreHours", 0.0)
            row["ram_gib_h"] += a.get("ramByteHours", 0.0) / GIB
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
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()

    if a.last:
        end = datetime.now(timezone.utc).replace(microsecond=0)
        start = end - parse_dur(a.last)
    elif a.start and a.end:
        start, end = parse_ts(a.start), parse_ts(a.end)
    else:
        sys.exit("要給 --start/--end，或 --last")

    snaps = load(a.snapshots, start, end)
    if not snaps:
        sys.exit(f"這段區間沒有任何快照：{start:%Y-%m-%dT%H:%M:%SZ} ~ {end:%Y-%m-%dT%H:%M:%SZ}")
    interval_s = snaps[0].get("interval_s", 300)

    # 快照自己也可能有斷層（這支備援掛掉的時候）。它的完整度要先講清楚，
    # 不然用一份殘缺的備援去補另一份殘缺的資料，錯得更難發現。
    expected = max(1, round((end - start).total_seconds() / interval_s))
    coverage = min(100.0, len(snaps) / expected * 100)

    by_dept, total_cpu, total_ram = allocate(snaps, interval_s)
    measured, measured_min = opencost_measured(a.compare, start, end) if a.compare else ({}, 0.0)

    if a.json:
        out = {
            "window": {"start": f"{start:%Y-%m-%dT%H:%M:%SZ}", "end": f"{end:%Y-%m-%dT%H:%M:%SZ}"},
            "basis": "declared",
            "snapshot_coverage_pct": round(coverage, 2),
            "samples": {"found": len(snaps), "expected": expected, "interval_s": interval_s},
            "departments": {d: {"cpuCoreHours": round(r["cpu_h"], 4),
                                "ramGiBHours": round(r["ram_gib_h"], 4),
                                "workloads": len(r["pods"])} for d, r in by_dept.items()},
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

    head = f"{'部門':<16}{'宣告核心小時':>14}{'宣告GiB小時':>14}{'份額%':>9}"
    if measured:
        head += f"{'實測份額%':>11}{'差距pp':>9}"
    if a.cpu_rate is not None and a.ram_rate is not None:
        head += f"{'金額':>12}"
    print(head)
    print("-" * 76)

    worst = 0.0
    for dept in sorted(by_dept, key=lambda d: -by_dept[d]["cpu_h"]):
        r = by_dept[dept]
        line = (f"{dept:<16}{r['cpu_h']:>14.4f}{r['ram_gib_h']:>14.4f}"
                f"{d_share_cpu[dept]:>9.2f}")
        if measured:
            ms = m_share_cpu.get(dept)
            if ms is None:
                line += f"{'—':>11}{'—':>9}"
            else:
                delta = d_share_cpu[dept] - ms
                worst = max(worst, abs(delta))
                line += f"{ms:>11.2f}{delta:>+9.2f}"
        if a.cpu_rate is not None and a.ram_rate is not None:
            line += f"{r['cpu_h'] * a.cpu_rate + r['ram_gib_h'] * a.ram_rate:>12.4f}"
        print(line)

    print(f"\n合計　　{total_cpu:.4f} 核心小時 / {total_ram:.4f} GiB 小時")
    if measured:
        m_cpu = sum(r["cpu_h"] for r in measured.values())
        # 兩邊各自除以自己實際涵蓋的時長，才是同一個基準
        d_hours = len(snaps) * interval_s / 3600.0
        m_hours = measured_min / 60.0 or d_hours
        d_rate, m_rate = total_cpu / d_hours, m_cpu / m_hours
        gap = (d_rate - m_rate) / m_rate * 100 if m_rate else 0.0
        print(f"實測　　{m_cpu:.4f} 核心小時（涵蓋 {measured_min:.1f} 分鐘）")
        print(f"換算成速率　宣告 {d_rate:.4f} 核 vs 實測 {m_rate:.4f} 核（宣告量法差 {gap:+.2f}%）")
        print(f"\n份額最大差距：{worst:.2f} 個百分點")
        print("宣告量法會低估「用量超過申請量」的工作負載——OpenCost 計價依 max(申請, 用量)，"
              "\n而快照只看得到申請量。差距多大取決於你的叢集有多少爆量的工作負載。")


if __name__ == "__main__":
    main()
