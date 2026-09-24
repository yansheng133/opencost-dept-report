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

print(f"\n{sum(R)}/{len(R)} 通過")
sys.exit(0 if all(R) else 1)
