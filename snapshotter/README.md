# snapshotter — 宣告量快照器

Prometheus 掛掉的時候，成本還是在發生，但你失去了分攤的依據。

這支程式每隔一段時間把每個 Running Pod 的 `requests` 與部門標籤存成 JSON。
它**只讀 API server**，完全不碰 Prometheus，所以兩者不會同時消失。真的斷線時，
這些快照就是第二份分攤依據（[降級階梯](../docs/data-gaps.md)的第 3 階）。

它刻意做得很笨：沒有資料庫、沒有相依套件、一個迴圈把檔案寫到磁碟。
備援系統比它保護的系統更複雜的話，就失去意義了。

## 部署

```bash
KCFG=<kubeconfig> bash deploy.sh           # 先看會做什麼（dry-run）
KCFG=<kubeconfig> bash deploy.sh --apply
```

跟 cost-report 一樣不需要 registry：程式碼放進 ConfigMap，掛進官方 Python 映像檔。
它跟 cost-report 共用 `cost-report` 這個 namespace，但兩者互不相依，可以只裝其中一個。

需要的權限只有 `pods` 與 `namespaces` 的 `get`/`list`——沒有任何寫入，也不碰 secret。

## 先在本機看看它會抓到什麼

不必先部署：

```bash
OUT_DIR=./snapshots python3 snapshot.py --once --kubectl <kubeconfig 路徑>
```

欄位跟叢集內模式完全一樣，所以本機收的快照可以跟正式的混著算。

## 算分攤依據

```bash
# 把叢集裡的快照取出來
pod=$(kubectl -n cost-report get pod -l app=cost-snapshotter -o jsonpath='{.items[0].metadata.name}')
kubectl -n cost-report cp $pod:/data ./snapshots

# 某一段區間，各部門的宣告資源小時數與份額
python3 fallback.py --snapshots ./snapshots --start 2026-09-24T05:00:00Z --end 2026-09-24T06:00:00Z

# 給單價就換算成金額（每 vCPU 小時、每 GiB 小時）
python3 fallback.py --snapshots ./snapshots --last 1h --cpu-rate 1.0 --ram-rate 0.25

# 跟 OpenCost 的實測值對照，量出這個方法差多少
python3 fallback.py --snapshots ./snapshots --last 20m --compare http://127.0.0.1:9003
```

`--compare` 要在**資料正常的期間**跑。沒有量過誤差的備援方案，跟沒有備援方案差不多——
真的要用的時候，你不知道該不該相信它算出來的數字。實測結果見
[docs/data-gaps.md](../docs/data-gaps.md)。

## 設定

| 環境變數 | 預設 | 說明 |
|---|---|---|
| `OUT_DIR` | `/data` | 快照存放目錄 |
| `INTERVAL` | `300` | 取樣間隔（秒）。5 分鐘足夠；間隔越短，短命的 Pod 越不會被漏掉 |
| `DEPT_LABEL` | `cost-center` | 部門標籤的鍵 |
| `ENV_LABEL` | `env` | 第二維度標籤的鍵 |
| `RETENTION_DAYS` | `35` | 保留天數（一個帳期加緩衝） |

實測用量：每份快照 6 KiB（36 個 Pod）。5 分鐘一次是每天 1.7 MiB，保留 35 天約 59 MiB，
所以 1Gi 的 PVC 綽綽有餘。取樣間隔設成 60 秒的話是每天 8.4 MiB，仍然沒問題。
行程本身 CPU 10m、記憶體 64Mi 以下。

## 測試

```bash
python3 test_snapshot.py     # 不需要叢集
```

只測兩件會直接影響帳單金額的事：資源單位換算，以及 Pod 實際宣告量的算法
（init container 峰值、原生 sidecar 的順序）。其他部分壞掉會很吵；這兩個壞掉會無聲地算錯錢。

## 已知限制

- **只看得到申請量，看不到實際用量。** 所以會低估爆量的工作負載——這是方法本身的界線，
  不是 bug。誤差多大要自己量（`--compare`）。
- **兩次取樣之間出生又死掉的 Pod 會整個漏掉。** 間隔越短漏得越少，但永遠不會是零。
  Job 很多的叢集要把間隔調短。
- **不含儲存成本。** PVC 的分攤要另外處理。
- 快照自己也可能有斷層（這支程式掛掉的時候）。`fallback.py` 會先報自己的完整度，
  不要拿一份殘缺的備援去補另一份殘缺的資料。
