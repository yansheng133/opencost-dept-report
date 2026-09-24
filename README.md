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
| 各部門應分攤金額 | 用量 × 單價 = 金額，附月推估。**閒置成本可切換「獨立列帳／按比例分攤」**，兩種做法總額相同 |
| 正式 vs 測試 | 用第二個標籤（例如 `env`）再切一次。測試環境常被當成「不用算」 |
| 每個工作負載付了多少、用了多少 | 申請量對實際用量，標出可回收的對象；也標出「效率低但其實在工作」的例外 |
| 無法分攤的成本 | 沒有部門標籤的部分有多大，就是標籤治理的量化指標 |
| 計價依據 | 單價、計價單位、本期用量；單價由資料反推，不寫死在程式裡 |
| **資料完整度** | 本期有多少時間真的量到了、缺在哪幾段。低於 99.5% 會在畫面最上方警告 |

## 這個服務做了什麼、沒做什麼

| 做了 | 沒做 |
|---|---|
| 查 OpenCost 的 allocation API，整理成部門視角 | 不自己算成本，也不改 OpenCost 的設定 |
| 背景定期更新，畫面讀快取 | 不寫入任何資料，沒有資料庫 |
| OpenCost 連不上時沿用上一份資料並在畫面標示 | 不會自己補資料（缺多少會講，補不補是政策） |
| 單價由資料反推（金額 ÷ 用量） | 不在程式裡寫死單價 |
| **沒有登入機制** | **不做認證與授權** |

最後一列最重要：**誰連得到這個服務，就看得到全部部門的成本**，跟 OpenCost 本身一樣。
畫面上的「只看某部門」只是看的方便，不是權限邊界。要限制存取，認證必須放在它前面，
見 [`k8s/ingress-example.yaml`](k8s/ingress-example.yaml)。

## 前提

- 叢集裡已經有 OpenCost 與 Prometheus，而且 OpenCost 算得出成本。
- 工作負載或 namespace 有用來分攤的標籤，預設是 `cost-center`（第二維度預設 `env`）。
- 看得到報表的人，你已經決定要不要限制。

## 部署

### A. 還沒有 registry（預設，不必建置映像檔）

程式碼放進 ConfigMap，掛進官方 Python 映像檔執行。

```bash
KCFG=<kubeconfig 路徑> bash deploy.sh          # 先看會做什麼（dry-run）
KCFG=<kubeconfig 路徑> bash deploy.sh --apply  # 部署
```

改完 `app/app.py` 或 `app/index.html` 重跑一次即可，腳本會更新 ConfigMap 並觸發滾動更新。

### B. 打包成映像檔

```bash
docker build -t <registry>/cost-report:0.1.0 .   # 注意目標叢集的架構（arm64／amd64）
docker push <registry>/cost-report:0.1.0
```

然後在 `k8s/cost-report.yaml` 裡換掉 `image`、刪掉 `command`、刪掉 `code` 這個 volume 與它的 volumeMount。

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
| `SYSTEM_NS_PREFIXES` | `kube-system,cattle-,…` | 這些 namespace 的元件不列進工作負載表（仍計入「無法分攤」） |
| `PROM_URL` | `http://prometheus-server.prometheus-system.svc.cluster.local:80` | 量資料完整度用。連不上只是少一塊資訊，報表照常 |
| `COVERAGE_JOB` | `opencost` | 用哪個 scrape job 判斷「這段有沒有量到」 |
| `PROM_TIMEOUT` | `5` | 問 Prometheus 的逾時。刻意比主逾時短：位址設錯不該拖慢報表本身 |

## 端點

| 路徑 | 用途 |
|---|---|
| `GET /` | 報表畫面 |
| `GET /api/report?window=24h` | JSON，給其他系統取用 |
| `GET /healthz` | liveness：行程活著就回 200（OpenCost 掛掉不重啟自己） |
| `GET /readyz` | readiness：成功抓到第一份資料才回 200 |

## 其他檔案

| 路徑 | 內容 |
|---|---|
| `examples/demo-workloads.yaml` | 示範用工作負載：日夜曲線、記憶體浪費、沒有標籤的三種情況 |
| `examples/precheck.sh` | demo 前的健康檢查，逐項 PASS／FAIL |
| `examples/export-by-dept.sh` | 依部門匯出 CSV，`--dept` 可出單一部門逐一工作負載 |
| `docs/opencost-notes.md` | **實機量測過的 OpenCost 行為與陷阱**，比這份 README 更值得先看 |
| `docs/data-gaps.md` | **監控斷線時成本怎麼分攤**：降級階梯、來源標記、出帳門檻 |
| `snapshotter/` | 宣告量快照器：只讀 API server 的第二份分攤依據，Prometheus 掛掉時還有數字可用 |

## 需求

- Python 3.11 以上（只用標準函式庫，沒有任何相依套件）
- Kubernetes 1.25 以上；在 RKE2 1.35 上驗證過
- OpenCost 1.121 系列；Prometheus 3.x

## 授權

Apache License 2.0，見 [LICENSE](LICENSE)。
