"""使用率折線圖可以選哪些取樣間隔。

這裡只放純函式，不碰網路，方便測試。白名單的界線都是實測出來的，不是猜的：

- **間隔不能小於 5 分鐘。** Prometheus 每 60 秒抓一次，1 分鐘的區段只有一筆樣本，
  用量全部算成 0；2 分鐘的區段少抓一筆就從 66.7% 掉到 49.9%——那是取樣造成的假下凹，
  畫出來會讓人以為部門真的少用了。
- **一張圖最多 288 個點。** 7 天每 30 分鐘一點（336 段）OpenCost 直接回 HTTP 500；
  24 小時每 5 分鐘一點（288 段）約 6 秒，還在可接受範圍。

擋在後端而不是只靠前端按鈕，是因為 API 誰都可以直接打：
一個沒有上限的 step 參數，等於讓任何人都能叫 OpenCost 跑一個會失敗的大查詢。
"""

STEP_MINUTES = {"5m": 5, "10m": 10, "15m": 15, "30m": 30,
                "1h": 60, "3h": 180, "6h": 360, "1d": 1440}
MIN_STEP_MINUTES = 5
MAX_POINTS = 288


def _candidates(span):
    """依區間長度給一組「看起來合理」的候選。太細或太粗的選項放上去只會讓人困惑。"""
    if span <= 180:
        return ["5m", "10m", "15m"]
    if span <= 2880:
        return ["5m", "10m", "30m", "1h"]
    return ["1h", "3h", "6h", "1d"]


def options(span):
    """這個區間長度（分鐘）可以選的間隔，由細到粗。"""
    if not span or span <= 0:
        return []
    out = []
    for s in _candidates(span):
        m = STEP_MINUTES[s]
        # 至少要兩個點才畫得出線；點太多 OpenCost 會失敗
        if m >= MIN_STEP_MINUTES and 2 <= span / m <= MAX_POINTS:
            out.append(s)
    return out


def default_step(span, configured=None):
    """沒有指定時用哪個間隔。configured 是 SERIES_STEPS 為這個區間設定的值。

    設定值不在白名單裡也照用——那是管理者刻意改的，不該被無聲換掉；
    但使用者自己從畫面上選的話，只能選白名單裡的。
    """
    if configured:
        return configured
    if not span:
        return None
    return ("5m" if span <= 180 else "10m" if span <= 2880 else
            "1h" if span <= 14400 else "1d")


def is_allowed(span, step):
    return step in options(span)


class Budget:
    """一段時間內，使用者自選間隔的查詢最多可以要 OpenCost 算幾段。

    為什麼需要：OpenCost 每算一段就對 Prometheus 開一批新連線，用完的連接埠要等
    TIME_WAIT（Linux 預設 60 秒）才能再用。實測 60 秒內連跑「24 小時每 5 分鐘」
    （288 段）和「指定某天每 5 分鐘」，第二個就失敗，錯誤是
    `connect: cannot assign requested address`——而且**同一時間的帳務報表查詢也一起失敗**。
    每個組合單獨看都合法，所以白名單擋不住，要看一段時間內的總量。

    背景更新（/api/report 每 60 秒）不經過這裡：那是固定的量，上線後實測沒出過錯。
    """

    def __init__(self, limit=300, period=60.0):
        self.limit, self.period = limit, period
        self._spent = []          # [(時間, 段數)]

    def try_take(self, n, now):
        """可以就記帳並回 (True, 0)；不行就回 (False, 幾秒後再試)。"""
        self._spent = [(t, k) for t, k in self._spent if now - t < self.period]
        used = sum(k for _, k in self._spent)
        if used and used + n > self.limit:
            # 要等到夠多的舊紀錄過期才放得下這一筆
            free, wait = self.limit - used, 0.0
            for t, k in self._spent:
                free += k
                wait = t + self.period - now
                if free >= n:
                    break
            return False, max(1, int(wait + 0.999))
        # 第一筆一律放行：單一查詢本身是否合理由白名單決定，不在這裡擋
        self._spent.append((now, n))
        return True, 0
