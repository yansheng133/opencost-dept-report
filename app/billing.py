#!/usr/bin/env python3
"""計價政策：價目表、出帳門檻、帳期封存、對帳。

這裡的東西跟「怎麼查 OpenCost」無關。它們是把**成本數字**變成**帳單**中間缺的那一段：

  單價是誰定的、什麼時候生效？        → 價目表
  這份數字可不可以拿去跟人收錢？       → 出帳門檻
  上個月的帳單，下個月還查得到嗎？     → 帳期封存
  分攤出來的總額跟實際花的錢對得上嗎？ → 對帳

這四件事沒有做，畫面上的數字就只是儀表板。做了，它才是帳務。

只用標準函式庫，而且刻意不碰網路與 Kubernetes——這樣才測得動（見 test_billing.py）。
"""
import hashlib
import json
import os
import time

# 出帳門檻的預設值。**這些數字是政策，不是技術常數**，所以可以從外部覆寫：
# 每家公司能容忍多少推估、多少對帳差異，是各自決定的事。
DEFAULT_THRESHOLDS = {
    "coverage_ok": 99.0,        # 量測覆蓋率：這個以上直接出帳
    "coverage_mark": 95.0,      # 這個以上可以出帳但要標示；以下擋住
    "estimated_ok": 1.0,        # 推估佔比：這個以下視為乾淨
    "estimated_mark": 10.0,     # 這個以下要標示；以上擋住
    "drift_ok": 0.5,            # 對帳差異（%）
    "drift_mark": 2.0,
    "unallocated_mark": 20.0,   # 無法分攤佔比，超過就提醒（不擋帳）
}

# 每一列成本的來源。帳單上要看得出哪些是量出來的、哪些是推估的。
BASIS_LABELS = {
    "measured": "實際量測",
    "interpolated": "鄰近時段推估",
    "declared": "宣告量推估",
    "prior_period": "前期比例推估",
    "policy": "政策分攤鍵",
}


# ── 價目表 ────────────────────────────────────────────────────────────────

def load_ratecard(path):
    """讀價目表。讀不到就回 None，呼叫端要能在沒有價目表的情況下繼續跑。"""
    try:
        with open(path) as f:
            card = json.load(f)
    except (OSError, ValueError):
        return None
    versions = card.get("versions") or []
    if not versions:
        return None
    card["versions"] = sorted(versions, key=lambda v: v.get("effective", ""))
    return card


def rate_at(card, when_iso):
    """取某個時間點生效的那一版單價。

    帳期中途改價的話，前後段要用不同單價——所以價目表是「一串有生效日的版本」，
    不是一組數字。用「生效日 <= 查詢時間」的最後一版，跟合約的算法一致。
    """
    if not card:
        return None
    day = (when_iso or "")[:10]
    chosen = None
    for v in card["versions"]:
        if v.get("effective", "") <= day:
            chosen = v
        else:
            break
    return chosen


# 一項資源佔總成本低於這個比例時，就算它的單價對不上，對帳單的影響也小到
# 不值得擋帳。單位是比例（0.01 = 1%）。
MATERIAL_SHARE = 0.01


def rate_check(configured, implied, cost_share=None, tolerance_pct=0.5):
    """比對「價目表寫的單價」與「OpenCost 實際在用的單價」。

    這兩個不一致代表有人只改了其中一邊——帳單會跟系統對不起來，而且不會有任何
    錯誤訊息。這是最容易發生、也最難事後追查的一種帳務錯誤，所以要當成出帳的
    硬性條件檢查，不是參考資訊。

    但是「反推單價」在金額很小的時候不可靠：查一個兩分鐘的區間時，儲存可能只花了
    0.0001，用這麼小的分母去除會得到 5% 以上的誤差——而那項只佔本期成本的 0.15%。
    拿它去擋整張帳單是假警報，而**假的「帳單不能發」比沒有這個檢查更傷信任**。

    所以帶入 cost_share（每項資源佔總成本的比例）：占比太小的項目照樣回報差異，
    但標成 minor 而不是 mismatch。原則是**資料薄到撐不起結論時就不要下結論**。
    """
    out = {}
    for key in ("cpu", "ram", "storage"):
        c, i = (configured or {}).get(key), (implied or {}).get(key)
        if c is None or i is None:
            out[key] = {"configured": c, "implied": i, "status": "unknown"}
            continue
        drift = abs(c - i) / i * 100 if i else (0.0 if c == 0 else 100.0)
        share = (cost_share or {}).get(key)
        row = {"configured": round(c, 6), "implied": round(i, 6),
               "driftPct": round(drift, 3)}
        if share is not None:
            row["costShare"] = round(share * 100, 3)
        if drift <= tolerance_pct:
            row["status"] = "ok"
        elif share is not None and share < MATERIAL_SHARE:
            # 差異是真的，但這項資源佔的錢太少，影響不到帳單
            row["status"] = "minor"
            row["impactPct"] = round(drift * share, 4)
        else:
            row["status"] = "mismatch"
        out[key] = row
    return out


# ── 對帳 ──────────────────────────────────────────────────────────────────

def reconcile(allocated_total, asset_total):
    """分攤出去的總額，對得上實際花掉的錢嗎？

    分攤的總和應該等於資產（節點、磁碟）的成本。對不上代表有成本沒有被分攤到，
    或者被重複計算。**對不上的帳單沒有人會付**，所以這是出帳前的必要檢查。
    """
    if not asset_total:
        return {"allocated": round(allocated_total or 0, 4), "assets": None,
                "driftPct": None, "status": "unknown"}
    drift = (allocated_total - asset_total) / asset_total * 100
    return {
        "allocated": round(allocated_total, 4),
        "assets": round(asset_total, 4),
        "driftPct": round(drift, 3),
        "status": "ok" if abs(drift) <= DEFAULT_THRESHOLDS["drift_ok"] else
                  ("mark" if abs(drift) <= DEFAULT_THRESHOLDS["drift_mark"] else "block"),
    }


# ── 出帳門檻 ──────────────────────────────────────────────────────────────

def _grade(value, ok, mark, higher_is_better=True):
    if value is None:
        return "unknown"
    if higher_is_better:
        return "ok" if value >= ok else ("mark" if value >= mark else "block")
    return "ok" if value <= ok else ("mark" if value <= mark else "block")


def billing_gate(coverage_pct, estimated_pct, recon, rates, unallocated_pct,
                 thresholds=None, has_data=True):
    """這份數字可不可以拿去收錢？

    回傳三種結論：
      ok    ——可以出帳
      mark  ——可以出帳，但帳單上必須標示推估的比例與方法
      block ——不應該直接出帳，要有人核准才放行

    **block 不是「不出帳」**，是「改成需要有人簽名為這份推估負責」。帳單還是要發，
    差別在於責任歸屬有沒有被記錄下來。
    """
    t = dict(DEFAULT_THRESHOLDS)
    t.update(thresholds or {})
    checks = []

    # 這一項要排在最前面：**沒有資料的時候，下面每一項都會因為分母是零而看起來完美。**
    # 覆蓋率 100%、推估 0%、無法分攤 0%、對帳差 0——然後系統告訴你可以出帳。
    # 剛裝好還沒貼標籤的叢集就是這個狀態，而那正是最不該說「一切正常」的時候。
    checks.append({
        "name": "本期有成本資料嗎",
        "value": None,
        "unit": "",
        "status": "ok" if has_data else "block",
        "detail": "有" if has_data else "本期完全沒有成本資料——先確認 OpenCost 有沒有在算，以及查詢區間對不對",
        "policy": "沒有資料就不能出帳，也不能說「一切正常」",
        "why": "分母是零的時候，其他每一項檢查都會看起來完美。這是最危險的一種假訊號。",
    })

    checks.append({
        "name": "量測覆蓋率",
        "value": coverage_pct,
        "unit": "%",
        "status": _grade(coverage_pct, t["coverage_ok"], t["coverage_mark"]),
        "policy": f"≥{t['coverage_ok']}% 直接出帳、≥{t['coverage_mark']}% 需標示、以下要核准",
        "why": "沒量到的時段成本照樣發生。覆蓋率低就是金額被少算，而且畫面上看不出來。",
    })
    checks.append({
        "name": "推估佔比",
        "value": estimated_pct,
        "unit": "%",
        "status": _grade(estimated_pct, t["estimated_ok"], t["estimated_mark"], higher_is_better=False),
        "policy": f"≤{t['estimated_ok']}% 視為乾淨、≤{t['estimated_mark']}% 需標示",
        "why": "推估可以做，但比例要講出來，爭議時才有得談。",
    })
    checks.append({
        "name": "對帳差異",
        "value": None if not recon else recon.get("driftPct"),
        "unit": "%",
        "status": "unknown" if not recon else recon.get("status", "unknown"),
        "policy": f"分攤總額 vs 資產成本，差 ≤{t['drift_ok']}% 為正常",
        "why": "對不上代表有成本沒被分攤，或被重複計算。對不上的帳單沒人會付。",
    })
    bad = [k for k, v in (rates or {}).items() if v.get("status") == "mismatch"]
    minor = [(k, v) for k, v in (rates or {}).items() if v.get("status") == "minor"]
    if bad:
        rate_status, detail = "block", "不一致：" + "、".join(bad)
    elif minor:
        rate_status = "mark"
        detail = "；".join(
            f"{k} 單價差 {v['driftPct']}%，但只佔本期成本 {v.get('costShare')}%"
            f"（對帳單的影響約 {v.get('impactPct')}%），不擋帳"
            for k, v in minor)
    elif not rates:
        rate_status, detail = "unknown", "沒有價目表，單價是反推的"
    else:
        rate_status, detail = "ok", "價目表與系統實際使用的單價相符"
    checks.append({
        "name": "單價一致性",
        "value": None,
        "unit": "",
        "status": rate_status,
        "detail": detail,
        "policy": "價目表寫的單價，必須等於系統實際計價用的單價",
        "why": "只改一邊不會有錯誤訊息，但帳單會跟系統永遠對不起來。",
    })
    checks.append({
        "name": "無法分攤佔比",
        "value": unallocated_pct,
        "unit": "%",
        "status": "ok" if (unallocated_pct or 0) <= t["unallocated_mark"] else "mark",
        "policy": f"超過 {t['unallocated_mark']}% 表示標籤治理需要處理",
        "why": "沒有部門標籤的成本，最後是平台團隊在付。這不擋帳，但它是治理的量化指標。",
    })

    order = {"block": 3, "mark": 2, "unknown": 2, "ok": 1}
    worst = max((order.get(c["status"], 1) for c in checks), default=1)
    verdict = {3: "block", 2: "mark", 1: "ok"}[worst]
    return {"verdict": verdict, "checks": checks}


# ── 帳期封存 ──────────────────────────────────────────────────────────────

def content_hash(payload):
    """對內容做雜湊，用來證明封存後沒有被改過。

    排序鍵值並固定分隔符號，否則同樣的內容換個順序就會得到不同的雜湊，
    「是否被竄改」這件事就驗不出來了。
    """
    blob = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()


def seal_day(seal_dir, day, payload):
    """把某一天的明細封存起來。已經封存過就**不覆寫**。

    帳單的要求是「結帳之後凍結」：同一天不管什麼時候查，都要得到同一份數字。
    所以這裡刻意不做「更新」——已經存在就回報它存不存在差異，讓人去處理，
    而不是安靜地把舊的蓋掉。安靜覆寫會讓「上個月的帳單」變成一個會漂移的東西。
    """
    os.makedirs(seal_dir, exist_ok=True)
    path = os.path.join(seal_dir, f"{day}.json")
    body = dict(payload)
    body["day"] = day
    digest = content_hash(body)

    if os.path.exists(path):
        try:
            with open(path) as f:
                old = json.load(f)
        except (OSError, ValueError):
            return {"status": "unreadable", "path": path}
        if old.get("sha256") == digest:
            return {"status": "unchanged", "path": path, "sha256": digest}
        return {"status": "conflict", "path": path,
                "sealedSha256": old.get("sha256"), "currentSha256": digest,
                "sealedAt": old.get("sealedAt")}

    body["sha256"] = digest
    body["sealedAt"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(body, f, ensure_ascii=False, sort_keys=True)
    os.replace(tmp, path)       # 先寫再改名：讀的人不會看到寫到一半的帳
    return {"status": "sealed", "path": path, "sha256": digest}


def read_seal(seal_dir, day):
    try:
        with open(os.path.join(seal_dir, f"{day}.json")) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def list_seals(seal_dir):
    """列出已封存的帳期，順便驗證每一份的雜湊還對不對。"""
    out = []
    try:
        names = sorted(os.listdir(seal_dir))
    except OSError:
        return out
    for name in names:
        if not name.endswith(".json"):
            continue
        doc = read_seal(seal_dir, name[:-5])
        if not doc:
            out.append({"day": name[:-5], "status": "unreadable"})
            continue
        stored = doc.get("sha256")
        body = {k: v for k, v in doc.items() if k not in ("sha256", "sealedAt")}
        out.append({
            "day": doc.get("day", name[:-5]),
            "sealedAt": doc.get("sealedAt"),
            "total": (doc.get("totals") or {}).get("total"),
            "coveragePct": (doc.get("coverage") or {}).get("pct"),
            "estimatedPct": (doc.get("basis") or {}).get("estimatedPct"),
            "gate": doc.get("gate"),          # 當天封存時的出帳結論，事後不會變
            "driftPct": (doc.get("reconciliation") or {}).get("driftPct"),
            # 重算一次雜湊：封存的意義在於「事後查得到而且沒被改過」，
            # 只存雜湊不驗證，等於沒有防竄改。
            "intact": stored == content_hash(body),
        })
    return out
