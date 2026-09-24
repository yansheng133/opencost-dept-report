# opencost-dept-report

把 [OpenCost](https://www.opencost.io/) 的原始數字，轉成**各部門該分攤多少、依據是什麼**的唯讀報表，
跑在叢集裡，不依賴任何人的筆電。

```
瀏覽器 ──▶ cost-report（本服務，快取 60 秒）──▶ OpenCost API ──▶ Prometheus
```

OpenCost 自己的 UI 是給平台團隊看的：它按 namespace 和工作負載呈現，沒有「這個部門要付多少」的視角，
而且**經 Rancher 的服務代理打開是空白頁**。這個專案補上那一層。

## 畫面上有什麼

| 區塊 | 回答什麼問題 |
|---|---|
| **各部門的使用率趨勢** | 折線圖：各部門申請的資源有沒有在用、什麼時候在用。看得出日夜曲線、批次作業，以及長期貼在低點的浪費 |
| **這份數字可以拿去收錢嗎** | 五項檢查（覆蓋率、推估佔比、對帳、單價一致性、標籤治理）逐項攤開，給出「可出帳／需標示／需核准」的結論 |
| **你現在走到哪一階** | 四階成熟度：看得到成本 → 分得出部門 → 數字可信 → 能真的收錢。用實際資料判斷位置，並指出下一階缺什麼 |
| 各部門應分攤金額 | 用量 × 單價 = 金額，附月推估。**閒置成本可切換「獨立列帳／按比例分攤」**，兩種做法總額相同 |
| 正式 vs 測試 | 用第二個標籤（例如 `env`）再切一次。測試環境常被當成「不用算」 |
| 每個工作負載付了多少、用了多少 | 申請量對實際用量，標出可回收的對象；也標出「效率低但其實在工作」的例外 |
| 無法分攤的成本 | 沒有部門標籤的部分有多大，就是標籤治理的量化指標 |
| 計價依據 | 單價、計價單位、本期用量；單價由資料反推，不寫死在程式裡 |
| **資料完整度** | 本期有多少時間真的量到了、缺在哪幾段、有多少金額是推估的、監控目標有沒有掉線過 |
| **帳期封存** | 已結帳凍結的日子、當天的出帳結論、雜湊完整性 |
| **下一步該做什麼** | 根據這個叢集的實際資料算出來的具體行動，含 namespace 名稱與金額 |

畫面本身的操作：**側邊快速連結**（自動從區塊生成，會跟著捲動高亮）、**每一節都有固定錨點**
（`#gate`、`#seals`…，可以把某一節的連結直接貼給同事）、**每一節可收合**（狀態記在瀏覽器裡，
下次打開還在），**過長的表格先只顯示前幾列**。寬螢幕是左側欄，窄螢幕會變成頂部的橫向列。

## 它示範的不只是數字

很多團隊卡在「知道要做成本分攤，但不知道怎麼做、怎麼呈現」。所以這個畫面刻意不只是把成本攤開，
而是把**一套 chargeback 實務**擺出來：

- 每個區塊都寫明「**這一節你要做的決定是什麼**」——閒置成本誰吸收、測試環境要不要收費、
  回收浪費的責任歸誰、沒人認領的成本最後誰付。這些都是政策決定，不是技術決定，
  而它們決定了各部門帳單上的數字。
- 帳單不是算出來就能發，**要先通過關卡**。畫面最上方把五項檢查攤開，
  包含每一項的政策門檻與「為什麼要檢查它」。
- 成熟度階梯說明**跳階不會成功**：標籤還沒治理好就開始收錢，時間會全花在吵數字。

## 這個服務做了什麼、沒做什麼

| 做了 | 沒做 |
|---|---|
| 查 OpenCost 的 allocation API，整理成部門視角 | 不自己算成本，也不改 OpenCost 的設定 |
| 背景定期更新，畫面讀快取 | 不寫入任何資料，沒有資料庫 |
| OpenCost 連不上時沿用上一份資料並在畫面標示 | 不會自己補資料（缺多少會講，補不補是政策） |
| 單價由資料反推（金額 ÷ 用量） | 不在程式裡寫死單價 |
| 價目表版本化（生效日、訂定者、原因） | 不自己決定單價該是多少 |
| 每天封存帳期並凍結，附雜湊防竄改 | 不會覆寫已封存的帳期（內容不同時回報衝突） |
| 跟資產成本對帳，差異超過門檻就擋帳 | 不自己修正差異 |
| 資料有斷層時跟快照器要宣告量補值並標示來源 | 快照器沒有那段資料時**不補**，只說明 |
| **沒有登入機制** | **不做認證與授權** |

最後一列最重要：**誰連得到這個服務，就看得到全部部門的成本**，跟 OpenCost 本身一樣。
畫面上的「只看某部門」只是看的方便，不是權限邊界。要限制存取，認證必須放在它前面，
見 [`k8s/ingress-example.yaml`](k8s/ingress-example.yaml)。

## 前提

- 叢集裡已經有 OpenCost 與 Prometheus，而且 OpenCost 算得出成本。
- 工作負載或 namespace 有用來分攤的標籤，預設是 `cost-center`（第二維度預設 `env`）。
- 看得到報表的人，你已經決定要不要限制。

## 部署

映像檔是多架構的（`linux/amd64` ＋ `linux/arm64`），直接部署即可：

```bash
KCFG=<kubeconfig 路徑> bash deploy.sh                    # 先看會做什麼（dry-run）
KCFG=<kubeconfig 路徑> bash deploy.sh --apply            # 部署報表服務
KCFG=<kubeconfig 路徑> bash snapshotter/deploy.sh --apply # 部署快照器（選用）
```

腳本會自己處理兩件容易踩的事：**選 StorageClass**（叢集不一定有預設的，沒有預設又有多個候選
就直接拒絕並列出可用的，而不是讓 PVC 無聲 Pending 到 rollout 逾時），以及**建立價目表 ConfigMap**。

換版本或換成自建的 registry：

```bash
IMAGE_TAG=0.3.0 KCFG=… bash deploy.sh --apply
IMAGE=myregistry.local/cost-report:1.2.3 KCFG=… bash deploy.sh --apply
```

### 自己建置映像檔

```bash
bash build-images.sh                 # 只建置不推送，驗證 Dockerfile
REGISTRY=<你的帳號> bash build-images.sh --push
```

多架構**一定要用 buildx 的 container driver**：預設的 docker driver 一次只產得出一個架構，
而且不會明講，只會安靜地推上去一個單架構的映像檔——別人在另一種 CPU 上拉下來才會發現。
腳本會自己建立 builder，推送完還會把實際的架構列出來給你核對。

### 程式碼為什麼不再放 ConfigMap

早期版本把 `.py` 放進 ConfigMap 掛載執行，好處是不必 registry。但那樣沒有版本、沒有
不可變性，也沒辦法告訴別人「你跑的是哪一版」。**價目表仍然是 ConfigMap**——單價是政策不是
程式，改價不該需要重新建置映像檔。

## 怎麼存取

| 方式 | 說明 |
|---|---|
| `kubectl -n cost-report port-forward svc/cost-report 8088:80` | 自己看，最快 |
| Rancher 服務代理 | `<Rancher>/k8s/clusters/<叢集 ID>/api/v1/namespaces/cost-report/services/http:cost-report:80/proxy/`。**沿用 Rancher 的登入與 Project 權限**，不必另開對外入口。網址結尾的斜線不能省，頁面用相對路徑取資料 |
| Ingress | 見 `k8s/ingress-example.yaml`，裡面有三種認證做法與稽核缺口 |

## 設定

| 環境變數 | 預設 | 說明 |
|---|---|---|
| `OPENCOST_URL` | `http://opencost.opencost.svc.cluster.local:9003` | OpenCost API |
| `CACHE_TTL` | `60` | 背景更新間隔（秒） |
| `WINDOWS` | `1h,24h,7d` | 畫面可選的區間。**其他值會被拒絕**，避免有人下超大查詢把 Prometheus 打爆 |
| `DEFAULT_WINDOW` | `24h` | 預設區間 |
| `CLUSTER_LABEL` | 空 | 畫面上顯示的叢集名稱 |
| `SERIES_STEPS` | `1h=5m,24h=1h,7d=6h` | 使用率折線圖每個區間的取樣間隔。點太密看不出趨勢，太疏會把日夜曲線抹平 |
| `SYSTEM_NS_PREFIXES` | `kube-system,cattle-,…` | 這些 namespace 的元件不列進工作負載表（仍計入「無法分攤」） |
| `PROM_URL` | `http://prometheus-server.prometheus-system.svc.cluster.local:80` | 量資料完整度用。連不上只是少一塊資訊，報表照常 |
| `COVERAGE_JOB` | `opencost` | 用哪個 scrape job 判斷「這段有沒有量到」 |
| `PROM_TIMEOUT` | `5` | 問 Prometheus 的逾時。刻意比主逾時短：位址設錯不該拖慢報表本身 |
| `RATECARD_PATH` | `/config/ratecard.json` | 價目表。沒有的話單價改用反推，畫面會註明 |
| `SEAL_DIR` | `/seals` | 帳期封存的存放目錄（要掛持久化的磁碟） |
| `SEAL_ENABLED` | `1` | 設成 `0` 關閉自動封存 |
| `SNAPSHOTTER_URL` | `http://cost-snapshotter.cost-report.svc.cluster.local:80` | 斷層補值的來源。留空就不補 |

## 端點

| 路徑 | 用途 |
|---|---|
| `GET /` | 報表畫面 |
| `GET /api/report?window=24h` | JSON，給其他系統取用 |
| `GET /api/seals` | 已封存的帳期清單，含雜湊完整性驗證 |
| `GET /api/seal?day=YYYY-MM-DD` | 單一帳期的完整封存內容 |
| `GET /healthz` | liveness：行程活著就回 200（OpenCost 掛掉不重啟自己） |
| `GET /readyz` | readiness：成功抓到第一份資料才回 200 |

## 其他檔案

| 路徑 | 內容 |
|---|---|
| `examples/demo-workloads.yaml` | 示範用工作負載：日夜曲線、記憶體浪費、沒有標籤的三種情況 |
| `examples/precheck.sh` | demo 前的健康檢查，逐項 PASS／FAIL |
| `examples/ui-smoke.js` | 畫面的冒煙測試（導覽、收合、錨點、三種寬度）。需要 Playwright，是唯一用到外部套件的東西 |
| `examples/export-by-dept.sh` | 依部門匯出 CSV，`--dept` 可出單一部門逐一工作負載 |
| `docs/opencost-notes.md` | **實機量測過的 OpenCost 行為與陷阱**，比這份 README 更值得先看 |
| `docs/data-gaps.md` | **監控斷線時成本怎麼分攤**：降級階梯、來源標記、出帳門檻 |
| `app/billing.py` | 計價政策：價目表、出帳門檻、帳期封存、對帳。刻意不碰網路，所以測得動 |
| `app/ratecard.json` | 價目表範例（版本化，含生效日與調整原因） |
| `app/test_billing.py` | 計價政策的測試，`python3 test_billing.py`，不需要叢集 |
| `snapshotter/` | 宣告量快照器：只讀 API server 的第二份分攤依據，Prometheus 掛掉時還有數字可用 |
| `build-images.sh` | 建置並推送兩個多架構映像檔（amd64／arm64） |
| `docs/dockerhub/` | Docker Hub 的倉庫描述與更新腳本（描述只放導向，不複製 README——那會變成第三個要同步的副本） |
| `Dockerfile`、`snapshotter/Dockerfile` | 兩個服務各自的映像檔定義 |

## 映像檔

| | |
|---|---|
| 報表服務 | `docker.io/yansheng133/cost-report:0.3.0` |
| 快照器 | `docker.io/yansheng133/cost-snapshotter:0.3.0` |
| 架構 | `linux/amd64`、`linux/arm64` |
| 基底 | `registry.suse.com/bci/python:3.12`，非 root（UID 1000）執行 |

## 需求

- Python 3.11 以上（只用標準函式庫，沒有任何相依套件）
- Kubernetes 1.25 以上；在 RKE2 1.35 上驗證過
- OpenCost 1.121 系列；Prometheus 3.x

## 授權

Apache License 2.0，見 [LICENSE](LICENSE)。
