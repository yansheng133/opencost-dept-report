# API

後端是 Kubernetes aggregated API。瀏覽器（Rancher 裡的頁面）經 Rancher 的 `/k8s/clusters/<叢集 ID>` 代理呼叫，
由那座叢集的 API server 驗證使用者並檢查 RBAC，再以 mTLS 轉給後端，使用者身分放在 `X-Remote-User`／`X-Remote-Group`。
後端只信任由 request-header CA 簽發、名稱在允許清單內的用戶端憑證帶來的身分。

- API group／版本：`api.costreport.yansheng133.github.io/v1alpha1`
- 所有資源都是叢集層級（部門橫跨多個 namespace）
- 只有讀取，沒有寫入

```
/apis/api.costreport.yansheng133.github.io/v1alpha1/<資源>[/<名稱>]
```

## 權限

API server 先檢查呼叫者對資源本身有沒有權限（`reports`、`series`、`seals`、`periods`、`config` 的 `get`／`list`）。
過了這一關，後端再判斷**看得到哪些部門**，方法是替呼叫者向 API server 送 SubjectAccessReview：

| 檢查 | 意思 |
|---|---|
| `get departments`（不帶名稱） | 看得到全部部門與全叢集的數字（閒置、無法分攤、對帳、監控目標、系統元件）。只有規則**沒有** `resourceNames` 時才會通過 |
| `get departments/<部門代號>` | 看得到這個部門 |

都不通過的人，回 403 `no_department`。結果以（使用者、群組、部門）為鍵快取 30 秒。

chart 預設建立三個 ClusterRole：

| ClusterRole | 給誰 |
|---|---|
| `cost-report-viewer-all` | 平台、FinOps、財務：全部部門＋全叢集數字 |
| `cost-report-department-<代號>` | 部門主管：只看自己的部門（每個部門一個，依 `departments` 設定產生） |
| `cost-report-base` | 只有讀取資源的權限，沒有任何部門。給要自己組合 `departments` 規則的人用 |

Rancher 的角色範本見 `deploy/rancher/`。

## 看得到的範圍

每一份回應都帶 `scope`：

```json
{ "scope": { "all": false, "departments": ["rd"] } }
```

`all` 是 `false` 時，回應會**拿掉**全叢集的部分（欄位設成 `null`）：`idle`、`unallocated`、`measuredTotal`、
`reconciliation`、`targets`、`unallocRows`、`basis.fill`。部門、工作負載、儲存、環境、使用率序列只留下範圍內的部門；
`total` 改成範圍內部門的合計。出帳關卡（`gate`）與覆蓋率（`coverage`）照常提供：它們說的是這份數字可不可信，不是別的部門花了多少。

## 錯誤

錯誤一律是 Kubernetes 的 `Status`，另外帶 `error` 欄位給頁面判斷：

```json
{
  "kind": "Status", "apiVersion": "v1", "status": "Failure",
  "code": 429, "reason": "TooManyRequests",
  "message": "OpenCost just ran a large query; retry in 12 seconds",
  "details": { "retryAfterSeconds": 12 },
  "error": { "code": "series_budget", "message": "…", "retryAfterSeconds": 12 }
}
```

| `error.code` | HTTP | 什麼時候 |
|---|---|---|
| `bad_window` | 400 | `window` 不在允許清單、`day` 格式不對 |
| `bad_step` | 400 | 取樣間隔不在這個區間的白名單裡；另帶 `options` |
| `no_department` | 403 | 呼叫者沒有任何部門的權限 |
| `not_found` | 404 | 路徑不存在、沒有這一天的封存 |
| `series_budget` | 429 | 一分鐘內自選取樣間隔的段數超過額度；另帶 `retryAfterSeconds` |
| `upstream` | 502 | OpenCost 沒有回傳這個區間的分段資料，或指定的那一天 OpenCost 查詢失敗 |
| `not_ready` | 503 | 還沒成功從 OpenCost 取得過資料 |
| `unauthenticated` | 401 | 請求不是經 API server 轉來的（沒有可信的身分） |

## 資源

### `GET config`（list）

頁面啟動時先問這個。

```json
{
  "version": "1.0.0",
  "cluster": "prod-east",
  "windows": ["1h", "24h", "7d"],
  "defaultWindow": "24h",
  "departments": [{ "id": "rd", "name": "研發部" }],
  "scope": { "all": false, "departments": ["rd"] },
  "labels": { "department": "cost-center", "environment": "env", "environments": ["prod", "dev"] },
  "features": { "seals": true, "volumeUsage": true, "snapshotter": true, "coverage": true },
  "currency": "TWD",
  "dataSince": "2026-10-31T01:20:00Z",
  "ingestLagMinutes": 5
}
```

`departments` 只列呼叫者看得到的。`dataSince` 是後端目前看到最早的資料時間（還沒有資料時是 `null`）。名稱來自 chart 的 `departments` 設定，沒設定的代號就顯示代號。

### `GET reports?window=24h` 或 `GET reports?day=2026-09-24`（list）

`window=24h` 是「到現在為止的最近 24 小時」：後端換算成絕對區間再問 OpenCost（OpenCost 自己的相對區間會對齊整點、包含還沒發生的時間）。
OpenCost 最新的幾分鐘會晚一點才寫入，所以區間結束在現在往前 `ingestLagMinutes`（預設 5）分鐘、再往下對齊到 5 分鐘（OpenCost 以 5 分鐘為單位寫入）。指定的日子還沒結束時，只算到那個時間點，而且報表超過 `ttlSeconds` 就重算；還沒開始的日子回 400 `bad_window`。實際查詢的區間在 `window.requested`（`start`、`end`）。

主報表。欄位跟舊版 `/api/report` 相同（見 `demo` 分支的 `app/app.py` `build_report`），差別：

- 多了 `scope`（見上）。
- `depts[]` 多了 `name`。
- `gate.checks[]` 不再帶中文說明，改成代號，頁面自己翻譯：
  ```json
  { "code": "coverage", "value": 99.6, "unit": "%", "status": "ok",
    "detail": { "code": "rates_minor", "args": { "items": [{ "resource": "storage", "driftPct": 5.6, "costShare": 0.15, "impactPct": 0.0084 }] } } }
  ```
  `code` 依序是 `has_data`、`coverage`、`estimated`、`reconciliation`、`rates`、`unallocated`。
  `detail.code`：`no_data`、`rates_mismatch`（`args.resources`）、`rates_minor`（`args.items[]`）、`rates_unknown`（沒有價目表）、
  `rates_no_version`（有價目表，但這段期間沒有適用的版本，例如期間沒有資料）、`rates_ok`。
  期間沒有資料時（`has_data` 是 `block`），`coverage`、`estimated`、`unallocated` 三項的 `status` 一律是 `unknown`、`value` 是 `null`：
  空的期間不會顯示「通過」。
  門檻在 `gate.thresholds`（`coverageOk`、`coverageMark`、`estimatedOk`、`estimatedMark`、`driftOk`、`driftMark`、`unallocatedMark`）。
- `basis.labels` 拿掉，頁面自己翻譯 `measured`／`declared`／`mixed`。
- `basis.fill.notes` 改成 `[{ "start", "end", "code": "no_snapshot" | "unreachable", "detail" }]`。
- `storage.rows[].depts[]` 是 `{ "id", "cost", "special" }`，`special` 是 `null`、`unallocated`、`unmounted` 或 `idle`（取代舊版的中文標籤）。
- `ccEnv[]` 的環境欄位改成 `{ "dept": "rd", "envs": { "prod": 1.2, "dev": 0.4 }, "other": 0.1 }`，環境值由 `labels.environments` 決定，不再寫死 prod／dev。
- `unallocRows[].system` 依 chart 的 `systemNamespaces` 判斷。
- `workloads[].cpuEff`／`ramEff` 在沒有申請量時是 `null`（OpenCost 原本回 1.0，看起來像「完美」）。
- `stale` 在報表超過三個 `ttlSeconds` 沒更新時也是 `true`。
- `reconciliation` 多了 `allocatedMinutes`、`assetsMinutes`。兩邊涵蓋的時間差超過 1% 時，`assets` 是換算成分攤時間長度後的金額，
  `assetsMeasured` 是 OpenCost 原本的數字，`scaled` 是 `true`。資產成本 ≤ 0（OpenCost 剛啟動時會出現）或讀不到時，
  `status` 是 `unknown`，`reason` 是 `assets_not_positive` 或 `assets_unavailable`。
- 外層的 `generatedAt`、`ageSeconds`、`ttlSeconds`、`stale`、`staleReason`、`windows` 不變。

### `GET series?window=24h&step=5m`（list）

使用者自選取樣間隔的使用率序列，形狀與 `reports` 的 `series` 相同（`step`、`stepMinutes`、`points[]`、`covered`、
`expected`、`options`、`defaultStep`、`ageSeconds`）。`step` 必須在 `options` 裡。等於預設間隔時直接回主報表那一份，不算額度。

### `GET seals`（list）

```json
{ "enabled": true, "scope": {…}, "seals": [{ "day": "2026-09-24", "sealedAt": "…", "total": 79.2, "coveragePct": 99.6,
  "estimatedPct": 0, "gate": "ok", "driftPct": -0.003, "intact": true }] }
```

`scope.all` 是 `false` 時，`total` 是範圍內部門在那天的合計，`driftPct` 是 `null`。`intact` 是重算雜湊的結果。

### `GET seals/<YYYY-MM-DD>`（get）

那一天的封存全文。`scope.all` 是 `false` 時只留範圍內的部門，`totals` 只剩 `total`，`reconciliation` 是 `null`。
雜湊（`sha256`）照原樣回傳，但只有全範圍的人能自己驗算。

### `GET periods`（list）

按月彙總的帳期，來源是封存（不是 Prometheus）。欄位同舊版 `/api/periods`，但 `reason` 改成代號
`{ "code": "tampered" | "missing_days" | "in_progress" | "day_blocked" | "day_marked" | "all_ok", "args": {…} }`
（`tampered` 帶 `days`，`missing_days` 帶 `missing`、`sealed`）；`scope.all` 是 `false` 時 `idle`、`unallocated` 是 `null`，
`depts` 只留範圍內的部門，`total` 是它們的合計。

### `GET departments`（list）

呼叫者看得到的部門，同 `config.departments`。`departments/<代號>` 是給 RBAC 判斷用的名稱，不另外提供內容。

## 健康檢查（不經 API server）

容器的 8080 埠（純 HTTP，只給 kubelet）：

| 路徑 | |
|---|---|
| `/healthz` | 行程活著就回 200 |
| `/readyz` | 成功從 OpenCost 取得過一次資料才回 200 |
| `/version` | 版本與 commit |
| `/metrics` | Prometheus 格式的後端自身指標（更新成功／失敗次數、上次成功時間、封存次數） |
