"""儲存空間：每個 PVC 申請了多少、實際用了多少、付的錢有多少是為空著的空間付的。

這裡只放純函式，不碰網路，方便測試。三份資料各有各的來源：

- **金額**：OpenCost。它照 PV 的容量 × 掛載時數 × 單價收費，跟實際用量無關。
  各部門的 allocation 裡有 `pvs`，鍵長得像 `cluster=rke2:name=pvc-…`。
- **容量與 PVC 名稱**：kube-state-metrics（kube_persistentvolume_capacity_bytes、…_claim_ref）。
- **實際用量**：volume_usage.py 在節點上量的（cost_report_pvc_used_bytes）。
  local-path 是 hostPath，kubelet 不量，所以只能自己量。

閒置金額用**區間內的最高用量**算，不用最後一刻的用量：
資料庫半夜清掉暫存檔之後看起來很空，但白天確實需要那麼多。拿最低點算會叫人砍到不夠用。
"""

GIB = 1024 ** 3

# 不是部門的歸屬，畫面上要講白話
SPECIAL = {"__unallocated__": "未標籤", "__unmounted__": "未掛載", "__idle__": "閒置"}


def pv_from_key(key):
    """OpenCost 的 pvs 鍵 → PV 名稱。`cluster=rke2:name=pvc-…` → `pvc-…`。"""
    for part in (key or "").split(":"):
        if part.startswith("name="):
            return part[len("name="):]
    return key or ""


def _depts(costs):
    """這個 PVC 的錢算給了誰。金額四捨五入後是 0 的不列——除非只有它一個。

    實測 OpenCost 會把同一個 PV 也掛在 __unallocated__ 底下，金額 0、byteHours 卻是幾秒鐘的量
    （大概是 Pod 換手那一瞬間），列出來會讓一個 PVC 看起來被兩個單位分攤。
    """
    rows = [{"id": d, "cost": round(c, 4), "label": SPECIAL.get(d)}
            for d, c in sorted(costs.items(), key=lambda kv: -kv[1])]
    shown = [x for x in rows if x["cost"] > 0]
    return shown or rows[:1]


def build(by_cc, usage=None, capacity=None, claims=None):
    """把三份資料併成畫面要的表。

    by_cc     OpenCost 依 cost-center 彙總的 allocation（accumulate=true 那一份）
    usage     {pv: {"used": 區間結束時, "peak": 區間最高, "errors": 讀不到的項目數}}
    capacity  {pv: 容量位元組}
    claims    {pv: (namespace, PVC 名稱)}
    """
    usage, capacity, claims = usage or {}, capacity or {}, claims or {}
    rows = {}
    for dept, alloc in (by_cc or {}).items():
        for key, pv in ((alloc or {}).get("pvs") or {}).items():
            cost = float((pv or {}).get("cost") or 0)
            byte_hours = float((pv or {}).get("byteHours") or 0)
            if cost <= 0 and byte_hours <= 0:
                # OpenCost 會在 __unallocated__ 底下列出 0 元的 PV（同一個 PV 已經算給部門了），
                # 留著只會讓一個 PVC 看起來被兩個單位分攤
                continue
            name = pv_from_key(key)
            r = rows.setdefault(name, {"pv": name, "cost": 0.0, "gibHours": 0.0, "depts": {}})
            r["cost"] += cost
            r["gibHours"] += byte_hours / GIB
            r["depts"][dept] = r["depts"].get(dept, 0.0) + cost

    out = []
    for name, r in rows.items():
        u = usage.get(name) or {}
        ns, pvc = claims.get(name) or (u.get("ns"), u.get("pvc"))
        cap = capacity.get(name)
        used, peak = u.get("used"), u.get("peak")
        basis = peak if peak is not None else used
        pct = (basis / cap) if (basis is not None and cap) else None
        # 用量超過容量時沒有閒置——local-path 不會擋超用，這種情況真的會發生
        idle = r["cost"] * max(0.0, 1 - pct) if pct is not None else None
        out.append({
            "pv": name, "ns": ns, "pvc": pvc,
            "depts": _depts(r["depts"]),
            "capacityBytes": cap,
            "usedBytes": used, "peakBytes": peak,
            "usedPct": round(pct, 4) if pct is not None else None,
            "over": bool(cap and basis is not None and basis > cap),
            "partial": bool(u.get("errors")),
            "cost": round(r["cost"], 4),
            "idleCost": round(idle, 4) if idle is not None else None,
            "gibHours": round(r["gibHours"], 2),
        })
    out.sort(key=lambda x: (-x["cost"], x["pv"]))

    measured = [x for x in out if x["idleCost"] is not None]
    total = round(sum(x["cost"] for x in out), 4)
    return {
        "rows": out,
        "total": total,
        # 閒置只加總量得到的那幾列；沒量到的不是 0，是不知道
        "idleCost": round(sum(x["idleCost"] for x in measured), 4) if measured else None,
        "measuredCost": round(sum(x["cost"] for x in measured), 4),
        "measured": len(measured),
        "count": len(out),
    }
