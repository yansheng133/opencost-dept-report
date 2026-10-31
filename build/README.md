# 容器映像檔

一個映像檔、一個執行檔 `cost-report`，Deployment 用 `args` 選擇元件（`server`、`snapshotter`、`volume-usage`）。

- 建置：`Containerfile`，SUSE BCI Rust 編譯，執行環境是 SUSE BCI Micro（沒有套件管理員、沒有 shell），以 UID 65532 執行。
- 架構：linux/amd64 與 linux/arm64，在各自的 GitHub Actions runner 上原生建置，不用 QEMU。
- 發布：`.github/workflows/image.yml`，打 `vX.Y.Z` tag 時推到 `ghcr.io/yansheng133/cost-report`，以 cosign keyless 簽章、每個架構附 SPDX SBOM。部署一律用 digest。
- 驗證簽章：

  ```bash
  cosign verify ghcr.io/yansheng133/cost-report@sha256:<digest> \
    --certificate-identity-regexp 'https://github.com/yansheng133/opencost-dept-report/.github/workflows/image.yml@refs/tags/v.*' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com
  ```

- 冒煙測試：`build/smoke-test.sh <image>` 檢查 `/healthz`、`/version`、非 root、收到 SIGTERM 會在 5 秒內正常結束。

GHCR 的新套件預設可能是私有的：第一次發布後到 GitHub 的 Packages → cost-report → Package settings 改成 Public，或在叢集設 `imagePullSecrets`。
