# cost-snapshotter

Prometheus 掛掉的時候，成本還是在發生——節點照跑、雲端照計費、機器照折舊——
但你失去了「這些錢該分給誰」的依據。

這支程式每 5 分鐘把每個 Running Pod 的**資源宣告量**（requests）與部門標籤存成一份 JSON。
它**只讀 Kubernetes API server，完全不碰 Prometheus**，所以兩者不會同時消失。
真的斷線時，這些快照就是第二份分攤依據。

- 只需要 `pods`／`namespaces`／`persistentvolumeclaims` 的 `get`/`list`，沒有任何寫入權限
- 儲存按 **PVC** 計算不按 Pod，共用同一個 PVC 不會重複計價
- 提供叢集內的查詢 API（`GET /allocation?start=&end=`）給報表服務取用
- 每份快照約 6 KiB；5 分鐘一次是每天 1.7 MiB

實測誤差（跟 OpenCost 的實際量測對照）：總量 −1.23%、部門份額最大差 3.06 個百分點。
方向是固定的——宣告量法會低估「用量超過申請量」的工作負載。

## 架構

`linux/amd64`、`linux/arm64`。基底 `registry.suse.com/bci/python:3.12`，非 root（UID 1000）執行。
只用 Python 標準函式庫，沒有任何相依套件。

## 用法

部署檔與完整說明在 GitHub：

**https://github.com/yansheng133/opencost-dept-report/tree/main/snapshotter**

授權：Apache-2.0
