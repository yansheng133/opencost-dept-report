#!/usr/bin/env python3
"""計價政策的測試：python3 test_billing.py（不需要叢集，也不需要任何套件）

測的是會直接影響「這張帳單能不能發、發出去對不對」的邏輯：
中途改價取哪一版、門檻怎麼判、封存會不會被安靜覆寫。
這些壞掉不會有錯誤訊息，只會讓帳單悄悄變成錯的。
"""
import json
import os
import shutil
import sys
import tempfile

import billing

R = []


def ok(cond, what, got=None):
    R.append(bool(cond))
    print(f"PASS  {what}" if cond else f"FAIL  {what}\n      得到 {got!r}")


CARD = {
    "currency": "成本單位",
    "versions": [
        {"effective": "2026-01-01", "cpu": 1.0, "ram": 0.25, "storage": 0.001, "reason": "初版"},
        {"effective": "2026-09-01", "cpu": 1.2, "ram": 0.30, "storage": 0.001, "reason": "硬體重新攤提"},
    ],
}

# ── 價目表：中途改價要取對版本 ────────────────────────────────────────────
ok(billing.rate_at(CARD, "2026-08-31T23:00:00Z")["cpu"] == 1.0, "生效日之前用舊版單價")
ok(billing.rate_at(CARD, "2026-09-01T00:00:00Z")["cpu"] == 1.2, "生效當天就用新版單價")
ok(billing.rate_at(CARD, "2026-12-31T00:00:00Z")["cpu"] == 1.2, "之後沿用最後一版")
ok(billing.rate_at(CARD, "2025-06-01T00:00:00Z") is None, "第一版生效前沒有單價可用（不可以硬取第一版）")
ok(billing.rate_at(None, "2026-09-01T00:00:00Z") is None, "沒有價目表時回 None，不可以炸")

# 版本順序顛倒的檔案也要能正確排序
tmpdir = tempfile.mkdtemp()
try:
    p = os.path.join(tmpdir, "rc.json")
    with open(p, "w") as f:
        json.dump({"versions": list(reversed(CARD["versions"]))}, f)
    loaded = billing.load_ratecard(p)
    ok(billing.rate_at(loaded, "2026-08-01T00:00:00Z")["cpu"] == 1.0,
       "檔案裡版本順序顛倒也要排對")
    ok(billing.load_ratecard(os.path.join(tmpdir, "nope.json")) is None, "檔案不存在回 None")
    with open(p, "w") as f:
        f.write("{壞掉的 JSON")
    ok(billing.load_ratecard(p) is None, "壞掉的 JSON 回 None，不可以炸")

    # ── 封存：不可以被安靜覆寫 ────────────────────────────────────────────
    seals = os.path.join(tmpdir, "seals")
    body = {"totals": {"total": 100.0}, "depts": [{"id": "mfg", "total": 60.0}]}
    r1 = billing.seal_day(seals, "2026-09-23", body)
    ok(r1["status"] == "sealed", "第一次封存成功", r1)

    r2 = billing.seal_day(seals, "2026-09-23", body)
    ok(r2["status"] == "unchanged", "同樣內容再封存一次：回 unchanged，不重寫", r2)

    changed = {"totals": {"total": 999.0}, "depts": [{"id": "mfg", "total": 60.0}]}
    r3 = billing.seal_day(seals, "2026-09-23", changed)
    ok(r3["status"] == "conflict", "內容變了：回 conflict，**不可以覆寫**", r3)
    ok(billing.read_seal(seals, "2026-09-23")["totals"]["total"] == 100.0,
       "衝突之後，磁碟上仍然是原本封存的那份")

    listed = billing.list_seals(seals)
    ok(len(listed) == 1 and listed[0]["intact"] is True, "列出封存並驗證雜湊完整", listed)

    # 清單要帶出當天的出帳結論。少了這個欄位畫面只會顯示「—」，
    # 看起來像「那天沒有結論」而不是「程式忘了給」——這種漏法沒有人會發現。
    billing.seal_day(seals, "2026-09-22", {"totals": {"total": 5.0}, "gate": "block",
                                           "reconciliation": {"driftPct": -0.02}})
    row = [x for x in billing.list_seals(seals) if x["day"] == "2026-09-22"][0]
    ok(row.get("gate") == "block", "封存清單要帶出當天的出帳結論", row)
    ok(row.get("driftPct") == -0.02, "封存清單要帶出當天的對帳差異", row)

    # 直接竄改檔案，驗證 intact 會變 False——這是防竄改唯一真正的證明
    doc = billing.read_seal(seals, "2026-09-23")
    doc["totals"]["total"] = 1.0
    with open(os.path.join(seals, "2026-09-23.json"), "w") as f:
        json.dump(doc, f, ensure_ascii=False, sort_keys=True)
    # 按日期取，不要用位置索引：清單多一筆就會指到別天去（剛剛就踩到了）
    tampered = [x for x in billing.list_seals(seals) if x["day"] == "2026-09-23"][0]
    ok(tampered["intact"] is False, "有人手改過封存檔 → intact 變 False", tampered)
    intact_other = [x for x in billing.list_seals(seals) if x["day"] == "2026-09-22"][0]
    ok(intact_other["intact"] is True, "沒被改過的那一天仍然是 intact（不可以連坐）", intact_other)
finally:
    shutil.rmtree(tmpdir, ignore_errors=True)

# ── 單價一致性 ────────────────────────────────────────────────────────────
rc = billing.rate_check({"cpu": 1.0, "ram": 0.25, "storage": 0.001},
                        {"cpu": 1.0, "ram": 0.25, "storage": 0.001})
ok(all(v["status"] == "ok" for v in rc.values()), "價目表與實際單價相符", rc)
rc2 = billing.rate_check({"cpu": 1.0}, {"cpu": 1.32})
ok(rc2["cpu"]["status"] == "mismatch", "單價對不上要抓出來", rc2)
ok(billing.rate_check({"cpu": 1.0}, {"cpu": None})["cpu"]["status"] == "unknown",
   "反推不出單價時是 unknown，不可以當成相符")

# 金額太小的項目：差異照報，但不該擋帳。
# 實機情境：查兩分鐘的區間時儲存只花了 0.0001，反推單價誤差 5.6%，
# 但它只佔本期成本的 0.15%——拿它擋整張帳單是假警報。
rc3 = billing.rate_check({"cpu": 1.0, "storage": 0.001},
                         {"cpu": 1.0, "storage": 0.000947},
                         {"cpu": 0.9985, "storage": 0.0015})
ok(rc3["storage"]["status"] == "minor", "占比極小的單價差異標成 minor 不是 mismatch", rc3["storage"])
ok(rc3["storage"]["impactPct"] < 0.01, "並且算出它對帳單的實際影響", rc3["storage"])
ok(billing.billing_gate(100.0, 0.0, billing.reconcile(100, 100), rc3, 5.0)["verdict"] == "mark",
   "只有 minor 時是「需標示」，不是「需核准」")

# 反過來：占比大的項目對不上，一定要擋
rc4 = billing.rate_check({"cpu": 1.0}, {"cpu": 1.32}, {"cpu": 0.9})
ok(rc4["cpu"]["status"] == "mismatch", "占比大的單價差異仍然是 mismatch", rc4["cpu"])
ok(billing.billing_gate(100.0, 0.0, billing.reconcile(100, 100), rc4, 5.0)["verdict"] == "block",
   "占比大的單價對不上 → 擋帳")

# 沒給占比時維持原本的嚴格判定（呼叫端沒提供資訊就不要自作主張放行）
ok(billing.rate_check({"cpu": 1.0}, {"cpu": 1.32})["cpu"]["status"] == "mismatch",
   "沒有占比資訊時不放寬")

# ── 對帳 ──────────────────────────────────────────────────────────────────
ok(billing.reconcile(100.0, 100.2)["status"] == "ok", "差 0.2% 算正常")
ok(billing.reconcile(100.0, 101.5)["status"] == "mark", "差 1.5% 要標示")
ok(billing.reconcile(100.0, 120.0)["status"] == "block", "差 20% 要擋")
ok(billing.reconcile(100.0, None)["status"] == "unknown", "沒有資產成本時是 unknown")

# ── 出帳門檻 ──────────────────────────────────────────────────────────────
g = billing.billing_gate(99.9, 0.0, billing.reconcile(100, 100), rc, 5.0)
ok(g["verdict"] == "ok", "全部達標 → 可以出帳", g["verdict"])

g2 = billing.billing_gate(86.31, 0.0, billing.reconcile(100, 100), rc, 5.0)
ok(g2["verdict"] == "block", "覆蓋率 86% → 擋住，要核准才放行", g2["verdict"])

g3 = billing.billing_gate(97.0, 0.0, billing.reconcile(100, 100), rc, 5.0)
ok(g3["verdict"] == "mark", "覆蓋率 97% → 可出帳但要標示", g3["verdict"])

g4 = billing.billing_gate(99.9, 0.0, billing.reconcile(100, 100), rc2, 5.0)
ok(g4["verdict"] == "block", "單價對不上 → 擋住（帳單會跟系統對不起來）", g4["verdict"])

g5 = billing.billing_gate(99.9, 0.0, billing.reconcile(100, 100), rc, 49.9)
ok(g5["verdict"] == "mark", "無法分攤 49.9% → 提醒但不擋帳", g5["verdict"])

# 空叢集：分母是零時每一項都會看起來完美，但絕對不能說「可以出帳」
empty = billing.billing_gate(100.0, 0.0, billing.reconcile(0, None), {}, 0.0, has_data=False)
ok(empty["verdict"] == "block", "完全沒有資料時不可以判定可出帳", empty["verdict"])
ok(empty["checks"][0]["name"] == "本期有成本資料嗎", "「有沒有資料」要排在所有檢查最前面")
ok(empty["checks"][0]["status"] == "block", "沒有資料這一項本身要是 block")
has = billing.billing_gate(99.9, 0.0, billing.reconcile(100, 100), rc, 5.0, has_data=True)
ok(has["verdict"] == "ok", "有資料且都達標時仍然是可出帳（新檢查不可以誤擋）", has["verdict"])

# ── 月報：把封存的帳期按月彙總 ────────────────────────────────────────────
tmp2 = tempfile.mkdtemp()
try:
    sd = os.path.join(tmp2, "seals")

    def seal(day, total, gate="ok", cov=100.0, est=0.0):
        billing.seal_day(sd, day, {
            "totals": {"total": total, "idle": total * 0.2, "unallocated": total * 0.3},
            "depts": [{"id": "mfg", "total": total * 0.3}, {"id": "rd", "total": total * 0.2}],
            "coverage": {"pct": cov, "gapMinutes": (100 - cov) * 14.4},
            "basis": {"estimatedTotal": est}, "gate": gate})

    # 2026-08 完整的一個月（31 天）
    for d in range(1, 32):
        seal(f"2026-08-{d:02d}", 10.0, gate="ok")
    months = billing.aggregate_periods(sd, today="2026-09-15")
    aug = [m for m in months if m["month"] == "2026-08"][0]
    ok(aug["sealedDays"] == 31 and not aug["missingDays"], "完整的月份沒有缺日", aug["sealedDays"])
    ok(abs(aug["total"] - 310.0) < 1e-6, "月總額是每日加總", aug["total"])
    ok(abs(aug["depts"]["mfg"] - 93.0) < 1e-6, "部門金額也是加總", aug["depts"])
    ok(aug["verdict"] == "ok", "每天都 ok → 整個月可出帳", aug["verdict"])

    # 缺一天：20 天的加總不是一個月的帳單
    billing.seal_day  # noqa
    sd2 = os.path.join(tmp2, "seals2")
    for d in list(range(1, 15)) + list(range(16, 32)):      # 少 8/15
        billing.seal_day(sd2, f"2026-08-{d:02d}", {
            "totals": {"total": 10.0}, "depts": [], "coverage": {"pct": 100.0},
            "basis": {"estimatedTotal": 0}, "gate": "ok"})
    aug2 = [m for m in billing.aggregate_periods(sd2, today="2026-09-15") if m["month"] == "2026-08"][0]
    ok(aug2["missingDays"] == ["2026-08-15"], "缺的日子要被列出來", aug2["missingDays"])
    ok(aug2["verdict"] == "block", "月份缺日 → 不可出帳（加總不是帳單）", aug2["verdict"])

    # 有一天 block，整個月就 block
    sd3 = os.path.join(tmp2, "seals3")
    for d in range(1, 32):
        billing.seal_day(sd3, f"2026-08-{d:02d}", {
            "totals": {"total": 10.0}, "depts": [], "coverage": {"pct": 100.0},
            "basis": {"estimatedTotal": 0}, "gate": "block" if d == 7 else "ok"})
    aug3 = [m for m in billing.aggregate_periods(sd3, today="2026-09-15") if m["month"] == "2026-08"][0]
    ok(aug3["verdict"] == "block", "有一天需要核准 → 整個月繼承", aug3["verdict"])

    # 當月還沒過完：還沒到的日子不算缺
    sd4 = os.path.join(tmp2, "seals4")
    for d in range(1, 11):
        billing.seal_day(sd4, f"2026-09-{d:02d}", {
            "totals": {"total": 10.0}, "depts": [], "coverage": {"pct": 100.0},
            "basis": {"estimatedTotal": 0}, "gate": "ok"})
    sep = [m for m in billing.aggregate_periods(sd4, today="2026-09-10") if m["month"] == "2026-09"][0]
    ok(not sep["missingDays"], "當月只算到今天為止，未來的日子不算缺", sep["missingDays"])
    ok(sep["complete"] is False and sep["verdict"] == "mark",
       "當月是累計值，不是完整帳單", (sep["complete"], sep["verdict"]))

    # 封存被竄改 → 整個月擋下來
    with open(os.path.join(sd4, "2026-09-03.json")) as f:
        doc = json.load(f)
    doc["totals"]["total"] = 999.0
    with open(os.path.join(sd4, "2026-09-03.json"), "w") as f:
        json.dump(doc, f, ensure_ascii=False, sort_keys=True)
    sep2 = [m for m in billing.aggregate_periods(sd4, today="2026-09-10") if m["month"] == "2026-09"][0]
    ok(sep2["verdict"] == "block" and "2026-09-03" in sep2["tampered"],
       "有封存檔被改過 → 整個月不可出帳", sep2.get("tampered"))
finally:
    shutil.rmtree(tmp2, ignore_errors=True)

print(f"\n{sum(R)}/{len(R)} 通過")
sys.exit(0 if all(R) else 1)
