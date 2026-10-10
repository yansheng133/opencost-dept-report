#!/usr/bin/env python3
"""取樣間隔白名單的測試：python3 test_steps.py（不需要叢集）

白名單是唯一擋住「叫 OpenCost 跑一個會失敗的大查詢」的地方，
也擋住會畫出假下凹的過細間隔。這兩件事壞掉都不會有錯誤訊息。
"""
import sys

import steps

R = []


def ok(cond, what, got=None):
    R.append(bool(cond))
    print(f"PASS  {what}" if cond else f"FAIL  {what}\n      得到 {got!r}")


H, D, W = 60, 1440, 10080
DAY = 1439.98   # ?day= 換成的區間是 00:00:00 到 23:59:59，不是整整 1440 分鐘

ok(steps.options(H) == ["5m", "10m", "15m"], "1 小時可選 5/10/15 分", steps.options(H))
ok(steps.options(D) == ["5m", "10m", "30m", "1h"], "24 小時可選 5/10/30 分與 1 小時", steps.options(D))
ok(steps.options(DAY) == steps.options(D), "指定某一天跟 24 小時的選項一樣", steps.options(DAY))
ok(steps.options(W) == ["1h", "3h", "6h", "1d"], "7 天可選 1/3/6 小時與 1 天", steps.options(W))

# 實測過的界線
ok(not steps.is_allowed(D, "1m"), "1 分鐘不行（每 60 秒才抓一次，用量全是 0）")
ok(not steps.is_allowed(D, "2m"), "2 分鐘不行（會有取樣造成的假下凹）")
ok(not steps.is_allowed(W, "30m"), "7 天每 30 分鐘不行（336 段，OpenCost 回 500）")
ok(steps.is_allowed(D, "5m"), "24 小時每 5 分鐘可以（剛好 288 段）")
ok(all(span / steps.STEP_MINUTES[s] <= steps.MAX_POINTS
       for span in (H, D, DAY, W, 3 * D) for s in steps.options(span)),
   "任何區間的任何選項都不超過 288 點")
ok(all(span / steps.STEP_MINUTES[s] >= 2
       for span in (H, D, W) for s in steps.options(span)),
   "每個選項至少有兩個點，畫得出線")

# 參數不是白名單裡的字串就擋
for bad in ("", "5", "5M", "300s", "1h;rm", None):
    ok(not steps.is_allowed(D, bad), f"不認得的 step 一律拒絕：{bad!r}")
ok(steps.options(None) == [] and steps.options(0) == [], "區間長度不明時沒有選項（不可以炸）")

# 預設值
ok(steps.default_step(D) == "10m" and steps.default_step(W) == "1h" and steps.default_step(H) == "5m",
   "預設：1 小時 5 分、24 小時 10 分、7 天 1 小時")
ok(steps.default_step(D, "1h") == "1h", "SERIES_STEPS 設定的值優先，不被無聲換掉")
ok(all(steps.default_step(s) in steps.options(s) for s in (H, D, DAY, W)),
   "預設值本身要在可選清單裡，否則按鈕列上沒有一顆是亮的")

# 額度：60 秒內使用者自選的查詢合計最多 300 段
b = steps.Budget(300, 60)
ok(b.try_take(288, 0) == (True, 0), "第一個大查詢（24 小時每 5 分鐘）放行")
r = b.try_take(288, 10)
ok(r[0] is False and r[1] == 50, "60 秒內第二個大查詢擋下，並告訴他 50 秒後再試", r)
ok(b.try_take(12, 10) == (True, 0), "小查詢塞得進剩下的額度就放行")
r = b.try_take(290, 61)
ok(r == (False, 9), "第一筆過期了，但第二筆（12 段）還在，12＋290 超過 300 → 擋，9 秒後第二筆過期", r)
r = b.try_take(288, 61)
ok(r == (True, 0), "12＋288 剛好 300，沒有超過 → 放行", r)
ok(b.try_take(288, 125) == (True, 0), "全部過期之後可以再跑大查詢")
b2 = steps.Budget(300, 60)
ok(b2.try_take(500, 0) == (True, 0), "額度空的時候單一查詢一律放行（合不合理由白名單決定）")

print(f"\n{sum(R)}/{len(R)} 通過")
sys.exit(0 if all(R) else 1)
