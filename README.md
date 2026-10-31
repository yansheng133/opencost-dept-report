# opencost-dept-report

把 OpenCost 算出來的叢集成本，依部門標籤拆成各部門的帳，直接在 Rancher 裡看；每個人只看得到自己有權限的部門。

> 2026-09 的 Python 版（單檔網頁、無認證）保留在 [`demo`](https://github.com/yansheng133/opencost-dept-report/tree/demo) 分支。`main` 是 Rust 重寫的版本，給正式環境用。

## 它做什麼

OpenCost 會算出每個工作負載花了多少錢，但要拿去跟部門收費，中間還差幾件事。這個專案補的就是這一段：

| 問題 | 做法 |
|---|---|
| 這筆錢算誰的？ | 依 Pod（或所在 namespace）的部門標籤彙總，Pod 的標籤優先 |
| 誰能看到哪些部門？ | 用 Kubernetes 的 RBAC 決定，跟叢集其他權限在同一個地方管 |
| 這份數字可以拿去收費嗎？ | 出帳關卡逐項檢查：有沒有資料、量測覆蓋率、推估佔比、對帳差異、單價一致性、無法分攤佔比 |
| 監控有斷層的時候怎麼辦？ | 量出缺了哪幾段，用宣告量快照補，並標成推估 |
| 上個月的帳，下個月還查得到嗎？ | 每天結帳一次並封存，附雜湊，不會被覆寫 |
| 申請的資源有沒有在用？ | 各部門使用率趨勢、每個工作負載的申請與實際用量、PVC 的申請容量與實際用量 |

名詞：

- **閒置**：節點有、但沒有任何工作負載申請的資源。可以獨立列帳（平台吸收），也可以按比例攤給部門。
- **無法分攤**：沒有部門標籤的工作負載。系統元件與沒標籤的應用會分開列。
- **覆蓋率**：要求的區間裡，有多少時間真的有量到資料。
- **封存**：把一個已經結束的 UTC 日的帳凍結成一個檔案。

## 運作方式

```
瀏覽器（Rancher 裡的「成本分攤」頁面）
  │  以使用者自己的 Rancher 登入
  ▼
Rancher 代理 → 下游叢集的 Kubernetes API server（驗證身分、檢查 RBAC）
  ▼  mTLS，使用者身分放在標頭
cost-report（aggregated API）──► OpenCost、Prometheus
  │  SubjectAccessReview：這個人可以看哪些部門？
  ├──► cost-report-snapshotter（宣告量快照，只讀 API server）
  └──► 封存（自己的 PVC）
```

- 後端是 Kubernetes aggregated API（`api.costreport.yansheng133.github.io/v1alpha1`），沒有 Ingress、沒有自己的登入。只接受 API server 的 proxy 用戶端憑證帶來的身分。
- 後端自己的權限只有兩項：讀 `kube-system/extension-apiserver-authentication`、建立 SubjectAccessReview。它不讀任何工作負載、不讀 Secret、不寫入叢集。
- 快照器只讀 namespace、PVC、Running Pod；PVC 用量量測（選配）只讀節點上 local-path 目錄的檔案大小。

API 的完整說明見 [docs/api.md](docs/api.md)。

## 安裝

需要：Rancher 2.15、下游叢集上已經裝好 OpenCost 與它讀的 Prometheus。OpenCost 的計價單位、要加的 scrape job 等注意事項見 [docs/opencost-notes.md](docs/opencost-notes.md)。

### 1. 後端（下游叢集）

```bash
helm install cost-report charts/cost-report -n cost-report --create-namespace -f my-values.yaml
kubectl get apiservice v1alpha1.api.costreport.yansheng133.github.io   # AVAILABLE 要是 True
```

`my-values.yaml` 至少要有：

```yaml
image:
  digest: sha256:<image workflow 產出的 digest>   # 一律用 digest，不用 tag
config:
  cluster: prod-east
  prometheusUrl: http://prometheus-server.prometheus-system.svc:80
  departments:
    - {id: mfg, name: 製造部}
    - {id: rd, name: 研發部}
rateCard:
  currency: TWD
  versions:
    - {effective: "2026-01-01", cpu: 1.0, ram: 0.25, storage: 0.001}
```

- `rateCard` 的單價必須跟 OpenCost 設定的一致（每 vCPU 小時、每 GiB 小時記憶體、每 GiB 小時儲存），出帳關卡會檢查。調價時**新增一個版本**，不要改舊的。價目表改了不用重啟。
- 沒有預設 StorageClass 的叢集，要設 `seals.persistence.storageClass` 與 `snapshotter.persistence.storageClass`。
- 用 local-path 的叢集可以開 `volumeUsage.enabled`，才看得到 PVC 的實際用量（Longhorn 等 CSI 儲存不需要）。它要掛 hostPath，namespace 不能套 Pod Security baseline／restricted。
- 所有設定見 [charts/cost-report/values.yaml](charts/cost-report/values.yaml)。

### 2. 權限

資源都是叢集層級，所以用 ClusterRoleBinding，或 Rancher 的叢集角色。

| ClusterRole | 給誰 | 看得到 |
|---|---|---|
| `cost-report-viewer-all` | 平台、FinOps、財務 | 全部部門，以及閒置、無法分攤、對帳、監控目標 |
| `cost-report-department-<代號>` | 部門主管 | 只有那個部門（每個 `config.departments` 一個） |
| `cost-report-base` | 要自己組權限的人 | 單獨綁沒有作用，要另外加部門規則 |

Rancher 的叢集角色範本在 [deploy/rancher/roletemplates.yaml](deploy/rancher/roletemplates.yaml)，套用到 local 叢集後，在叢集的「成員」頁面加人。

判斷方式：後端替呼叫者問 API server「可不可以 `get departments/<代號>`」。規則**沒有** `resourceNames` 就是全部部門。所以權限的稽核、撤銷都跟其他 Kubernetes 權限一樣，30 秒內生效。

### 3. Rancher UI extension

在 Rancher 的 Extensions → Manage Repositories 加入 `https://yansheng133.github.io/opencost-dept-report`，安裝 **Cost allocation**（`cost-report-ext`）。左側選單會出現「成本分攤」，有兩頁：

- **部門成本**：出帳關卡、各部門金額（閒置獨立列帳或按比例分攤）、使用率趨勢、正式與測試環境、工作負載、儲存空間、無法分攤、資料可信度、待處理事項、計價依據。
- **帳期**：每日封存與按月彙總。

頁面會在使用者看得到的叢集裡找有裝 cost-report 的；有好幾座時可以切換。開發與建置見 [extension/README.md](extension/README.md)。

## 資料可信度的設計

- **覆蓋率的分母是要求的區間**，不是 OpenCost 回傳的區間。分母選錯的話，資料最少的時候看起來最完美。
- **斷層用宣告量補**：快照器每 5 分鐘記下每個 Pod 的資源申請量與部門。斷層期間的成本＝宣告的資源小時數 × 當時生效的單價，帳上標成推估。宣告量法會低估用量超過申請量的工作負載，誤差可以用 `cost-report fallback --compare` 在資料完整的期間量出來。詳見 [docs/data-gaps.md](docs/data-gaps.md)。
- **對帳用含閒置的總額**跟資產成本比。不含閒置會看起來短少兩成以上。
- **封存不覆寫**：同一天再封一次，內容不同就回報衝突。列表時重算雜湊，被改過的會標出來，整個月因此不能出帳。月報缺任何一天也不能出帳。
- **使用率序列不內插**：沒有資料的區段讓折線斷開。
- **自選取樣間隔有額度**：OpenCost 連續跑細的分段查詢會用完連往 Prometheus 的連接埠，連帳務報表都會失敗，所以一分鐘內有段數上限。

## 限制

- 一座叢集一套後端，一個副本（封存寫在 ReadWriteOnce 的 PVC）。
- 帳期以 UTC 日切。
- 只讀：價目表、部門名稱、門檻都在 chart 的 values 裡改。
- 部門代號就是標籤值，只接受小寫英數與 `-`（它也是 ClusterRole 名稱的一部分）。
- 社群專案，不在 SUSE 的支援範圍內。

## 開發

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

cd extension && yarn install --frozen-lockfile && yarn test && yarn build-pkg cost-report-ext

helm lint charts/cost-report -f charts/cost-report/ci/ci-values.yaml
```

本機試後端（不需要叢集，但要連得到 OpenCost）：

```bash
COST_REPORT_DEV_SCOPES='finance=*;rd-manager=rd' COST_REPORT_CONFIG=config.json \
  cargo run -p cost-report -- server
curl -H 'X-Remote-User: rd-manager' localhost:8080/apis/api.costreport.yansheng133.github.io/v1alpha1/reports
```

`COST_REPORT_DEV_SCOPES` 會無條件相信 `X-Remote-User`，只能在本機用；在叢集裡設了會拒絕啟動。

發布：打 `vX.Y.Z` tag 後，`image` workflow 在 x86-64 與 ARM64 上各自原生建置映像檔、推到 GHCR、用 cosign 簽章並附 SBOM；`publish` workflow 把 extension 與 chart 加進 `gh-pages` 的 Helm repo。

## 目錄

| 路徑 | 說明 |
|---|---|
| `crates/cost-report-core` | 純邏輯，不碰網路：價目表、出帳關卡、對帳、封存與月報、覆蓋率、使用率序列、儲存、權限範圍、宣告量、PVC 用量量測 |
| `crates/cost-report` | 執行檔：`server`（aggregated API）、`snapshotter`、`volume-usage`、`fallback` |
| `extension` | Rancher UI extension `cost-report-ext` |
| `charts/cost-report` | Helm chart |
| `deploy/rancher` | Rancher 叢集角色範本 |
| `Containerfile`、`build/` | 容器映像檔（SUSE BCI Micro、非 root） |
| `docs` | API、OpenCost 的注意事項、資料斷層的處理 |

## 授權

Apache License 2.0，見 [LICENSE](LICENSE)。
