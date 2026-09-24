# OpenCost 實機量測筆記

這些不是讀文件推論的，是在實機上量出來的。環境：Rancher 管理的 RKE2 1.35 單節點（arm64），
OpenCost 1.121.2、Prometheus 3.14，chart 取自 SUSE Application Collection。量測日期 2026-09。

## 計價

**單位**（用三個不同的單價反推，所以能分辨）

| 欄位 | 單位 | 驗算 |
|---|---|---|
| CPU | 每 vCPU 小時 | `cpuCoreHours` 0.075 × 單價 1 = `cpuCost` 0.075 |
| 記憶體 | 每 GiB 小時（2^30） | `ramByteHours` ÷ 2^30 = 0.0375 GiB·h × 0.25 = 0.009375，API 回 0.00938 |
| 儲存 | 每 GiB 小時 | `pvByteHours` ÷ 2^30 = 0.3 GiB·h × 0.001 = 0.0003 |

- 確認自訂單價有生效：`cpuCost ÷ cpuCoreHours` 要等於你設定的值。等於 **0.031611** 代表還在用上游預設。
- **反推單價時只能用有部門標籤的項目**：`__idle__` 有金額但用量欄位是 0，混進分母會把單價灌高
  （實測：CPU 單價被算成 1.32，實際是 1.0）。

**計價依 max(申請量, 實際用量)**

| 情況 | request | usage | 計價依據 |
|---|---|---|---|
| 只申請不使用 | 0.5 | 0 | 申請量 |
| 用量超過申請 | 0.2 | 0.30 | 實際用量（`cpuEfficiency` 會大於 1） |

節點的容量被「申請量」佔住，別人就排不進來，所以即使沒在用也要付。

## Allocation API 的行為

- `aggregate` 可用 `namespace`、`controller`、`pod`、`container`、`label:<鍵>`，可逗號組合。
- **標籤鍵含連字號可以用**：`label:cost-center`、`filter=label[cost-center]:"mfg"` 都正常。
- **Pod 標籤優先於 namespace 標籤**：把某個 Pod 標成別的部門，它的成本就算到那個部門，
  這是「跨部門計費」最簡單的示範方式。
- **`window=1h` 是「從目前整點起算」，不是最近 60 分鐘。** 19:10 查會得到 19:00～19:10，
  `minutes` 只有 10.6。要「最近 60 分鐘」就給 RFC3339 的絕對起訖。
- **比對兩次查詢一定要用絕對時間**，相對 window 在兩次查詢之間會移動，造成假差異。
- 回傳的 `start`／`end` 是資料實際涵蓋的區間，會比你要求的短。
- **整台節點重開後，所有 Pod 會同時短暫出現負成本**：`start` 比 `end` 還晚、`minutes` 是負的，
  約 5 分鐘後自己恢復。不是時鐘問題。demo 前剛重開過，要先等一下再看。

## 閒置成本與無法分攤

- `includeIdle=true&shareIdle=false`：閒置成本獨立成 `__idle__`。
- `includeIdle=true&shareIdle=true`：`__idle__` 消失，按比例攤給各項目。
- **兩者總額必須一致**（實測差 0.001%）。這是 chargeback 對帳的第一道檢查。
- `aggregate=label:<鍵>` 時，`__unallocated__` 就是所有沒有該標籤的 namespace 的合計。
  用同一段絕對 window 查 `aggregate=namespace` 加總，應該完全對上。
- 交叉查詢時可能看到 `__unallocated__/<namespace>`，金額為 0：那是 OpenCost 替**未掛載的 PVC**
  預留的佔位項目，不是部門成本掉進去。
- **標籤不會回溯**：查詢區間跨越「加上標籤」的時間點時，前段會落在 `__unallocated__`，
  同一個工作負載也可能出現兩列（一列未標籤、一列已標籤）。

## 跟資產成本對帳

`/assets` 這支 API 給的是節點與磁碟的實際成本，可以拿來驗證「分攤出去的錢」有沒有漏。

- **對帳一定要用含閒置的分攤總額**（`includeIdle=true`）。不含的話會看起來短少兩成以上
  （實測 −26.4%），然後你會花一個下午找一個根本不存在的漏洞：閒置就是節點買了沒人用的
  那一塊，它當然算在資產成本裡。
- 實測對得非常準：`includeIdle=true` 的分攤總額 **142.6174** vs 資產總額 **142.6179**，
  差 **−0.0004%**。這個數字本身就是很好的說服素材——它證明分攤是完整的。
- `aggregate=type` 其實**不會真的彙總**：回來的是逐一資產（兩顆磁碟各一列），
  要自己按 `type` 欄位加總。
- 資產分三類：`Node`（絕大部分）、`Disk`、`ClusterManagement`（自架是 0）。

## 自己寫 PromQL 時的重複計算

OpenCost 也會輸出 `kube_pod_container_resource_requests`（`job="opencost"`），和 kube-state-metrics
的同名 metric **序列一樣多**。不加篩選直接 `sum`，宣告量會變成 **2 倍**。

| 問題 | 修正 |
|---|---|
| 兩個 job 重複 | 加 `job="kubernetes-service-endpoints"` |
| 已結束（Succeeded／Failed）的 Pod 也被算進去 | 乘上 `kube_pod_status_phase{phase=~"Pending\|Running"} == 1` |
| 跟節點 Allocated 還差一點 | 正常：這個 metric 只算一般容器，排程器取「一般容器總和」與「init container 最大值」較大者 |

用 allocation API 不受影響。**對外給任何 PromQL 範例之前，先跑一次 `count by (job) (<metric>)`。**

## 部署上的坑

- **MCP server 在 chart 裡預設是開的**（上游 README 說預設停用，兩者矛盾）。用不到就關。
- **OpenCost 找 Prometheus 靠 chart 預設位址**（`prometheus-server.prometheus-system:80`）。
  Prometheus 的 release 名稱或 namespace 一改，就要在 values 明確指定位址。
- Prometheus 要自己加 OpenCost 的 scrape job。
- 叢集沒有 StorageClass 時，Prometheus 的 PVC 會卡在 Pending。
- **OpenCost UI 經 Rancher 服務代理是空白頁**（API 則正常），所以要看 UI 只能本機轉送，
  或者改用別的前端——這個專案就是為此而生。
- 多叢集：Prometheus chart 沒有專用的 external labels 鍵，用 `server.global.external_labels`；
  只寫這一項時 Helm 會與原本的 `server.global` 合併。OpenCost 預設用 `cluster_id` 這個標籤名稱。

## 資料連續性

- 最早資料時間：`time() - prometheus_tsdb_lowest_timestamp_seconds`。
- 連續性 = `count_over_time(up{job="opencost"}[span]) × 抓取間隔 ÷ span`。
- **分母要從 opencost target 自己的第一筆樣本算起**，不是 Prometheus 的起點，
  否則剛裝好時會報出不存在的斷層。
- 筆電上的環境：睡眠、闔蓋、VM 關機都會造成斷層，而且補不回來。
  OpenCost 不會替沒量到的時段推估——這點對客戶反而是可信度的來源。
