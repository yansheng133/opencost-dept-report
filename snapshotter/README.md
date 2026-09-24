# snapshotter — 宣告量快照器

Prometheus 掛掉的時候，成本還是在發生，但你失去了分攤的依據。

這支程式每隔一段時間把每個 Running Pod 的 `requests`、每個 PVC 的儲存宣告量，
連同部門標籤存成 JSON。它**只讀 API server**，完全不碰 Prometheus，所以兩者不會同時消失。
真的斷線時，這些快照就是第二份分攤依據（[降級階梯](../docs/data-gaps.md)的第 3 階）。

它刻意做得很笨：沒有資料庫、沒有相依套件、一個迴圈把檔案寫到磁碟。
備援系統比它保護的系統更複雜的話，就失去意義了。

快照可以離線算（`fallback.py`），也可以由同叢集的服務直接查（下方的
[查詢 API](#查詢-api)）。兩條路走的是同一份計算程式碼，不會給出兩種答案。

## 部署

```bash
KCFG=<kubeconfig> bash deploy.sh           # 先看會做什麼（dry-run）
KCFG=<kubeconfig> bash deploy.sh --apply
```

程式碼在映像檔 `docker.io/yansheng133/cost-snapshotter:0.2.0` 裡（amd64／arm64）。
要自己建置就跑 `bash ../build-images.sh --push`。

**StorageClass**：`deploy.sh` 會自己找——有預設的就用預設，沒有預設但只有一個就用那一個，
有多個就要你指定 `STORAGE_CLASS=<名稱>`。這一段是踩過才加的：RKE2 裝了 local-path
也不一定會標成 default，留空的話 PVC 會一直 Pending，而 rollout 要等到逾時才吐一句
「no persistent volumes available」，看不出真正的原因。
**改 StorageClass 要先把舊的 PVC 刪掉**（這個欄位建立後不可變）。
它跟 cost-report 共用 `cost-report` 這個 namespace，但兩者互不相依，可以只裝其中一個。

需要的權限只有 `pods`、`namespaces`、`persistentvolumeclaims` 的 `get`/`list`——
沒有任何寫入，也不碰 secret。少了 `persistentvolumeclaims` 不會報錯，只會讓快照裡的
`pvcs` 永遠是空陣列，而空陣列跟「這個叢集真的沒有 PVC」長得一模一樣——儲存成本
會無聲地算成 0。升級舊部署時這條特別容易漏。

deploy.sh 還會建一個 `cost-snapshotter` Service（80 → 8080），給查詢 API 用。
它沒有 Ingress、沒有 NodePort：那個介面回的是分攤依據，只該給叢集內的服務看。

## 先在本機看看它會抓到什麼

不必先部署：

```bash
OUT_DIR=./snapshots python3 snapshot.py --once --kubectl <kubeconfig 路徑>
```

（這支程式只用標準函式庫，所以本機直接 `python3` 跑得起來，不必先建置映像檔。）

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

## 儲存成本：為什麼按 PVC 算，不按 Pod 算

快照裡有兩個跟儲存有關的欄位，用途不一樣，**別拿錯**：

| 欄位 | 位置 | 意思 | 拿來做什麼 |
|---|---|---|---|
| `pv` | 每個 Pod 一個 | 這個 Pod 掛了多少儲存（位元組） | 查「誰掛了什麼」 |
| `pvcs` | 快照根層，每個 PVC 一列 | 這個 PVC 宣告了多少（位元組） | **部門分攤只能用這個** |

原因是儲存的成本屬於 PVC，不屬於掛它的 Pod。兩個 Pod 掛同一個 RWX 的 PVC 時，
磁碟只有一份、帳也只該收一份；把 Pod 的 `pv` 加起來會收兩次錢。
這種錯不會有任何徵兆——總額照樣出得來，只是那個部門多付了。
Deployment 擴成 3 個副本共用一個 PVC，帳單就會憑空變成三倍。

部門歸屬也一樣看 PVC 自己：先看 PVC 的標籤，沒有才退回它所在 namespace 的標籤。
不去看「掛它的 Pod 屬於哪個部門」，否則兩個不同部門的 Pod 掛同一個 PVC 時，
同一顆磁碟會有兩種歸屬，而你事後查不出當時是照哪一種收的。

`fallback.py` 的表會多一欄「宣告GiB小時(儲存)」，`--compare` 會拿它跟 OpenCost 的
`pvByteHours` 對照。這一欄的誤差方向跟 CPU／記憶體不同：PVC 宣告多少就佔多少磁碟，
兩邊應該幾乎一樣（lab 實測 8.0000 vs 8.0000 GiB，差 0.00%）。**差很多通常不是方法誤差，
是有 PVC 沒被算到**——多半是 RBAC 少了 `persistentvolumeclaims`。

金額欄要另外給 `--pv-rate`。只給 `--cpu-rate`／`--ram-rate` 的話金額不含儲存，
表頭會標成「金額(不含儲存)」，免得有人拿一個少算儲存的數字去出帳。

## 查詢 API

`kubectl cp` 是給人用的。cost-report 在報表中途發現資料斷層時，需要的是當下就問得到，
所以快照器自己開一個很小的唯讀 HTTP 介面（預設 8080，`LISTEN_PORT` 可改，設 0 就不開）。
它跑在背景執行緒，只讀磁碟上已經寫好的快照，不碰 API server——被打爆也只是它自己變慢。

```bash
curl "http://cost-snapshotter.cost-report/allocation?start=2026-09-24T07:45:00Z&end=2026-09-24T07:50:00Z"
```

```json
{
  "window": {"start": "2026-09-24T07:45:00Z", "end": "2026-09-24T07:50:00Z"},
  "basis": "declared",
  "snapshotCoveragePct": 100.0,
  "samples": {"found": 1, "expected": 1, "intervalSeconds": 300},
  "departments": {
    "it": {"cpuCoreHours": 0.0125, "ramGiBHours": 0.0469, "pvGiBHours": 0.1667, "workloads": 2}
  }
}
```

- 沒有部門標籤的歸到 `__unallocated__`，不會被丟掉。
- `snapshotCoveragePct` 是「找到的快照數 ÷ 期望數」，上限 100，算法跟 `fallback.py` 一樣。
  低於 99 就表示**這份備援自己也有缺漏**，不要拿它去補另一份殘缺的資料。
- 區間內**完全沒有快照時回 404**，不是回一份全零的成功結果。呼叫端必須分得出
  「沒有資料」和「資料是零」：前者要往降級階梯再下一階，後者可以直接出帳。
- `start`／`end` 格式不對回 400。
- `GET /healthz` → 200 `ok`，給 readinessProbe 用。

**livenessProbe 刻意不用 /healthz。** 它探的是心跳檔還新不新，也就是「還寫得出快照嗎」。
HTTP 在背景執行緒，取樣迴圈卡死它照樣回 200——改成打 /healthz 等於換成一個
永遠會過的檢查。readinessProbe 用 /healthz 是另一件事：那只決定要不要把流量導進 Service。

在本機試：

```bash
OUT_DIR=./snapshots python3 -c "import snapshot, time; snapshot.serve(18080); time.sleep(300)"
curl "http://127.0.0.1:18080/allocation?start=...&end=..."
```

## 設定

| 環境變數 | 預設 | 說明 |
|---|---|---|
| `OUT_DIR` | `/data` | 快照存放目錄 |
| `INTERVAL` | `300` | 取樣間隔（秒）。5 分鐘足夠；間隔越短，短命的 Pod 越不會被漏掉 |
| `DEPT_LABEL` | `cost-center` | 部門標籤的鍵 |
| `ENV_LABEL` | `env` | 第二維度標籤的鍵 |
| `RETENTION_DAYS` | `35` | 保留天數（一個帳期加緩衝） |
| `LISTEN_PORT` | `8080` | 查詢 API 的埠；設成 `0` 就不開 |

實測用量：每份快照 6 KiB（36 個 Pod）。5 分鐘一次是每天 1.7 MiB，保留 35 天約 59 MiB，
所以 1Gi 的 PVC 綽綽有餘。取樣間隔設成 60 秒的話是每天 8.4 MiB，仍然沒問題。
行程本身 CPU 10m、記憶體 64Mi 以下。

## 測試

```bash
python3 test_snapshot.py     # 不需要叢集
```

只測會直接影響帳單金額的事：資源單位換算、Pod 實際宣告量的算法（init container 峰值、
原生 sidecar 的順序）、PVC 去重、schema 1 的相容性、區間查詢的邊界與完整度封頂。
其他部分壞掉會很吵；這幾個壞掉會無聲地算錯錢。

**每一項都弄壞過一次，確認它真的會 FAIL。** 這次就靠這個流程抓到一個漏洞：
把 `G` 的倍數從 `1e9` 改成 `1e6`（PVC 的 `storage` 很常寫成 `10G` 而不是 `10Gi`），
整份測試竟然全過——原本只測了 `Gi` 和 `M`，沒測 `G`。現在補上了。

> **macOS 上的坑**：系統的 `python3` 設了 `sys.pycache_prefix`
> （`~/Library/Caches/com.apple.python`），bytecode 快取不在 `__pycache__` 裡，
> `ls` 看不到。改動如果**沒有改變檔案大小**、而且發生在同一秒內，Python 會沿用舊的
> `.pyc`，於是測試跑的是上一版的程式碼。植入／還原 bug 驗證時特別容易中招。
> 結果可疑就先 `rm -rf "$HOME/Library/Caches/com.apple.python$PWD"`。

## 已知限制

- **只看得到申請量，看不到實際用量。** 所以會低估爆量的工作負載——這是方法本身的界線，
  不是 bug。誤差多大要自己量（`--compare`）。
- **兩次取樣之間出生又死掉的 Pod 會整個漏掉。** 間隔越短漏得越少，但永遠不會是零。
  Job 很多的叢集要把間隔調短。
- **儲存只看 PVC 的宣告量，不看實際用了多少。** `spec.resources.requests.storage` 是
  「跟 StorageClass 要了多少」，不是「寫進去多少」。多數 provisioner 就是照宣告量配置，
  所以這兩個數字通常一樣；thin provisioning 的後端則會高估。
- **還沒 Bound 的 PVC 也會被算進去。** 它還沒有對應的 PV，嚴格說成本還沒發生。
  一個 PVC 長期 Pending 本來就是該修的事，所以這裡選擇讓它出現在帳上而不是消失——
  但對帳對不起來的時候，記得先看有沒有 Pending 的 PVC。
- **看不到 PV／StorageClass 的層級差異。** 所有儲存都當成同一種單價。
  SSD 跟冷儲存混用的叢集要自己按 StorageClass 分開計價，這支程式沒有做。
- 快照自己也可能有斷層（這支程式掛掉的時候）。`fallback.py` 會先報自己的完整度，
  不要拿一份殘缺的備援去補另一份殘缺的資料。
