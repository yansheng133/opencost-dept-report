# cost-report

依部門標籤把 [OpenCost](https://www.opencost.io/) 的成本拆成各部門的帳，在 Rancher 裡看；每個人只看得到自己有權限的部門。

- 原始碼、Helm chart、Rancher UI extension：<https://github.com/yansheng133/opencost-dept-report>
- 正本映像檔在 GHCR：`ghcr.io/yansheng133/cost-report`。這裡是同一個 digest 的副本，cosign 簽章一併複製。

## 標籤

| 標籤 | 內容 |
|---|---|
| `1.9.0`、`1.9` | 目前的版本。Rust 版：Kubernetes aggregated API、宣告量快照器、PVC 用量量測，一個執行檔（`server`／`snapshotter`／`volume-usage`／`fallback`） |
| `1.0.0`、`1.0` | 第一個 Rust 版，保留供舊部署使用。1.1～1.8 只在 GHCR，沒有複製到這裡 |
| `1.*-rc.*` | 測試版，只有 arm64，不要用 |
| `0.x`、`latest` | 2026-09 的 Python 版（單檔網頁、**沒有登入機制**），不再更新。`latest` 停在 0.10.0，不會指向 1.x |

部署請用 Helm chart，並以 digest 指定映像檔：

```bash
helm repo add cost-report https://yansheng133.github.io/opencost-dept-report
helm install cost-report cost-report/cost-report -n cost-report --create-namespace -f my-values.yaml
```

## 映像檔

- `linux/amd64`、`linux/arm64`，在各自架構上原生建置
- 基底 SUSE BCI Micro，沒有 shell 與套件管理員，以 UID 65532 執行
- cosign keyless 簽章，每個架構附 SPDX SBOM：

```bash
cosign verify docker.io/yansheng133/cost-report:1.9.0 \
  --certificate-identity-regexp 'https://github.com/yansheng133/opencost-dept-report/.github/workflows/image.yml@refs/tags/v.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

Apache-2.0。社群專案，不在 SUSE 的支援範圍內。
