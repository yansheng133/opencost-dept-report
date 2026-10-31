# 成本分攤 Rancher UI extension

`extension/` 是成本分攤的 Rancher UI extension。裝好之後，Rancher 側邊選單會多一個頂層產品**成本分攤**，底下有兩頁：

| 頁面 | 內容 |
|---|---|
| 部門成本 | 一段期間（1 小時、24 小時、7 天，或指定某一天）的部門成本：出帳關卡、各部門金額、使用率趨勢、各環境成本、工作負載、儲存空間、無法分攤的成本、資料可信度、待處理事項、計價依據 |
| 帳期 | 每天封存的帳期，以及由封存彙總出來的月報；點某一天會開那一天的部門成本 |

頁面只負責顯示。數字全部由後端計算（見 `../docs/api.md`），頁面不自己算金額。

| 項目 | 值 |
|---|---|
| 目標 Rancher 版本 | 2.15 |
| Extensions API | v3 |
| `@rancher/shell` | `3.0.13`（固定版本，不用 `^`） |
| Node.js | 24 以上 |
| 套件管理 | Yarn 1（1.22.22） |
| Extension 套件名稱 | `cost-report-ext`（`pkg/cost-report-ext`） |
| 語言 | 繁體中文、英文（跟著 Rancher 的語言設定；繁體中文由 extension 自己加進語言選單） |

## 頁面怎麼找到後端

後端是 Kubernetes aggregated API（API group `api.costreport.yansheng133.github.io`，版本 `v1alpha1`），裝在 Rancher 管理的叢集上。每一座有 OpenCost 的叢集都可以裝一套，所以頁面不寫死叢集，而是在執行時尋找（`utils/target.ts`）：

1. 讀 Rancher 的叢集清單（`/v1/management.cattle.io.clusters`），只會看到使用者自己看得到的叢集。
2. 對每一座叢集問 API group 的 discovery（`/k8s/clusters/<叢集 ID>/apis/api.costreport.yansheng133.github.io/v1alpha1`）。回 200 就是有裝。
3. 對有裝的叢集問 `config`：
   - 200：可以看。`config.departments` 是空的而且不是全範圍時，代表這個人沒有任何部門的權限，頁面顯示「沒有部門可以看」，不再問報表。
   - 403：API server 的權限不允許。
   - 503：後端暫時沒有回應（重啟中，或還沒取得過 OpenCost 的資料）。
4. 超過一座時，頁首出現叢集選單。上次選的叢集記在這個瀏覽器（`localStorage` 的 `cost-report-cluster`）；瀏覽器不允許儲存時，每次都選第一座看得到的。

所有請求都經 Rancher 的 `/k8s/clusters/<叢集 ID>` 代理，用使用者自己的 Rancher 登入（瀏覽器送 Rancher 的 session cookie）。extension 不保存、不轉送任何 token。API 只有讀取，不需要 CSRF token。

## 看得到什麼

後端依每個人的權限決定看得到哪些部門（`docs/api.md` 的〈權限〉），每一份回應都帶 `scope`。頁面照它顯示：

- **全範圍**（`scope.all` 是 `true`）：全部部門，加上閒置、無法分攤、對帳、監控目標與系統元件。
- **部分部門**：只有那幾個部門。頁面最上方寫明看得到哪些部門，閒置、無法分攤、對帳、監控目標整塊不顯示，閒置成本的切換也不出現。出帳關卡與覆蓋率照常顯示，因為它們說的是數字可不可信。
- **沒有部門**：顯示請管理者給部門角色的說明。

部門名稱來自後端的 `config.departments`（Helm chart 的 `departments` 設定），沒設定名稱的部門顯示代號。顏色依部門在清單裡的順序決定，所以同一個部門在每個期間、每一頁都是同一個顏色。

## 幾個刻意的做法

這些都是舊版網頁實際出過錯之後的規則，改的時候要留著：

- **時間用看的人所在的時區**。API 給的是 UTC，直接切字串的話台灣看到的時間會差 8 小時。
- **使用率折線在沒有資料的地方斷開**，不連線、不內插；X 軸照真實時間排，不照點的序號，否則 3 天的空白看起來跟 6 小時一樣。還沒過完的最後一段畫成虛線。
- **取樣間隔記在網址的 `?step=`**，分享連結時對方看到同一張圖；每分鐘自動更新時保留使用者選的間隔。後端回 429 時顯示要等幾秒，圖維持原本的間隔。
- **沒量到不是 0**：儲存空間沒有量測資料時寫「沒有量」，閒置金額寫「—」。
- **讀不到報表時講清楚**：畫面寫明「沒有數字不代表成本是零」，不會留下一頁空表格。
- 欄位說明在**滑鼠移過去與鍵盤聚焦時都會出現**（只綁滑鼠的話，觸控與鍵盤使用者看不到）。
- 帳期月報的結論與原因由後端給代號，頁面翻譯；缺的日子收成區間顯示（例如「09-01 ~ 09-20（20 天）」）。

## 目錄

```
extension/
├── package.json              開發用 app（private）
├── yarn.lock
├── jest.config.js            單元測試與元件測試設定
├── test/                     jest 用的 .vue 轉換與 i18n 替身
└── pkg/cost-report-ext/      extension 本體
    ├── package.json          extension 中繼資料與 catalog annotation
    ├── index.ts              進入點：註冊產品、路由與繁體中文語系
    ├── product.ts            頂層產品與側邊選單
    ├── config/product.ts     產品、頁面、路由名稱
    ├── config/backend.ts     API group、逾時、更新間隔、表格列數
    ├── composables/          找後端（useBackend）、各區塊共用的翻譯與部門資訊、文字
    ├── pages/Report.vue      部門成本
    ├── pages/Periods.vue     帳期
    ├── components/           區塊（sections/）與共用元件（說明提示、長條圖、收合區塊…）
    ├── utils/                純函式：找後端、API、格式、範圍、折線圖幾何、待處理事項、文字
    ├── utils/__tests__/      純函式的單元測試
    ├── components/__tests__/ 頁面掛載測試（假的後端回應）
    ├── styles/report.scss    樣式（cr- 開頭，只用 Rancher 的主題變數，深淺色都適用）
    └── l10n/                 zh-hant.yaml、en-us.yaml
```

## 開發與建置

需要 Node.js 24 與 Yarn 1.22.22。

```bash
cd extension
yarn install --frozen-lockfile
yarn test                      # 單元測試與頁面掛載測試
yarn lint
yarn build-pkg cost-report-ext # 產出 dist-pkg/cost-report-ext-<版本>/
```

`build-pkg` 會做 TypeScript 型別檢查（包含測試檔），型別錯誤會讓建置失敗。

本機沒有 Node.js 24 時，可以用 SUSE BCI 的容器建置：

```bash
docker run --rm -v "$PWD":/w -w /w registry.suse.com/bci/nodejs:24 \
  sh -c 'npm install -g yarn@1.22.22 && yarn install --frozen-lockfile && yarn test && yarn build-pkg cost-report-ext'
```

### 在 Rancher 裡試

1. 建置後在 `extension/` 執行 `yarn serve-pkgs`，它會在 `http://127.0.0.1:4500` 提供建好的檔案，並印出 extension 的網址。
2. Rancher 右上角使用者選單 → Preferences 打開 **Enable Extension developer features**。
3. Extensions → ⋮ → **Developer Load**，貼上 `serve-pkgs` 印出的網址（要帶 `http://`），需要的話勾 Persist。

頁面要看到資料，叢集上必須已經裝好後端（Helm chart，見專案根目錄的 README）。

## 測試

| 測試 | 內容 |
|---|---|
| `utils/__tests__/target.test.ts` | 找後端：只問有 API group 的叢集、存取狀態（可看、沒有部門、沒權限、沒回應）、記住的叢集 |
| `utils/__tests__/api.test.ts` | 錯誤代號（後端的 `error.code` 與 API server 自己的拒絕分開）、期間的查詢字串、日期驗證 |
| `utils/__tests__/series.test.ts` | 折線斷開、X 軸照時間、虛線的最後一段、末端標籤推開、選擇的間隔 |
| `utils/__tests__/scope.test.ts` | 部門名稱與顏色、範圍決定顯示哪些區塊、部門篩選 |
| `utils/__tests__/actions.test.ts` | 待處理事項與各表格下方的觀察，含空叢集與部分範圍 |
| `utils/__tests__/describe.test.ts` | 出帳檢查、待處理事項、月報原因的文字（含後端兩種 `rates_minor` 形狀） |
| `utils/__tests__/format.test.ts` | 數字、容量、當地時間、日期收成區間、表格收合 |
| `components/__tests__/pages.test.ts` | 用假的後端回應掛載兩頁：全範圍每一區都出現、部門範圍不出現全叢集數字、沒有部門、報表失敗、取樣間隔 429、帳期 |

頁面掛載測試用 `test/vue-transform.js` 轉換 `.vue`，翻譯用 `test/use-i18n-stub.ts` 替身（回傳翻譯鍵與參數），所以斷言比對的是翻譯鍵，不是實際文字。
