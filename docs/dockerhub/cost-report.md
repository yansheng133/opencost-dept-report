# cost-report

把 [OpenCost](https://www.opencost.io/) 的原始數字，轉成**各部門該分攤多少、依據是什麼**的唯讀報表，
跑在 Kubernetes 叢集裡。

```
瀏覽器 ──▶ cost-report（快取 60 秒）──▶ OpenCost API ──▶ Prometheus
```

- 各部門應分攤金額（閒置成本可切換獨立列帳／按比例分攤，兩者總額相同）
- 各部門使用率趨勢折線圖（申請的資源有沒有在用、什麼時候在用）
- 出帳關卡：覆蓋率、推估佔比、對帳差異、單價一致性、標籤治理，結論分「可出帳／需標示／需核准」
- 帳期封存：每天結帳凍結成不可變 JSON，附 SHA-256
- 資料有斷層時跟 `cost-snapshotter` 要宣告量補值並標示來源

**沒有登入機制**——誰連得到就看得到全部部門的成本，跟 OpenCost 本身一樣。
要限制存取，認證必須放在它前面。

## 架構

`linux/amd64`、`linux/arm64`。基底 `registry.suse.com/bci/python:3.12`，非 root（UID 1000）執行。
只用 Python 標準函式庫，沒有任何相依套件。

## 用法

完整的部署檔、設定說明與實機量測筆記都在 GitHub：

**https://github.com/yansheng133/opencost-dept-report**

```bash
kubectl apply -f https://raw.githubusercontent.com/yansheng133/opencost-dept-report/main/k8s/cost-report.yaml
```

（上面那個 manifest 預設會去掛一個名為 `cost-report-ratecard` 的 ConfigMap 當價目表，
並且需要一個 PVC 存放封存的帳期。建議照 repo 裡的 `deploy.sh` 部署，它會處理 StorageClass 的挑選。）

授權：Apache-2.0
