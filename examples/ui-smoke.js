#!/usr/bin/env node
/**
 * 畫面的冒煙測試：導覽與收合真的能用嗎？
 *
 *   node examples/ui-smoke.js [網址]      預設 http://127.0.0.1:8099/
 *
 * 需要 Playwright（`npm i -D playwright` 或 `npx playwright`）。這是唯一一個需要
 * 外部套件的東西，所以刻意放在 examples/ 而不是主程式旁邊——服務本身仍然零相依。
 *
 * 為什麼有這支：收合功能上線時我只看了一節就覺得好了，結果十節裡有五節一按下去
 * 整個消失、連「展開」的按鈕都被藏起來，再也打不開。原因是這一頁同時存在
 * .head 與 .sec-head 兩種標題容器，而 CSS 只列了其中一個。
 * **能把使用者鎖在無法復原狀態的功能，一定要每一個都測過，不能抽樣。**
 */
const { chromium } = require("playwright");

const URL = process.argv[2] || "http://127.0.0.1:8099/";
const results = [];

function check(ok, what, detail) {
  results.push(ok);
  console.log(ok ? `PASS  ${what}` : `FAIL  ${what}\n      ${detail || ""}`);
}

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  const warnings = [];
  page.on("pageerror", e => errors.push(String(e)));
  page.on("console", m => { if (m.type() === "warning") warnings.push(m.text()); });

  await page.goto(URL, { waitUntil: "networkidle" });
  await page.waitForTimeout(2200);

  const ids = await page.$$eval("section[id]", ns => ns.map(n => n.id));
  check(ids.length >= 8, `找到 ${ids.length} 個可導覽的區塊`);
  check(await page.$$eval("#toc a", n => n.length) === ids.length,
        "目錄項目數等於區塊數（目錄是從 DOM 生成的，不該對不上）");

  // 每一節都要測。收合是個能把人鎖在外面的功能，抽樣測不夠。
  for (const id of ids) {
    await page.click(`#${id} .fold`);
    await page.waitForTimeout(110);
    const folded = await page.evaluate(sid => {
      const sec = document.getElementById(sid), f = sec.querySelector(".fold");
      return { collapsed: sec.classList.contains("collapsed"),
               height: Math.round(sec.getBoundingClientRect().height),
               btnVisible: !!(f && f.offsetParent !== null),
               h2Visible: !!(sec.querySelector("h2") && sec.querySelector("h2").offsetParent !== null) };
    }, id);
    check(folded.collapsed && folded.btnVisible && folded.h2Visible && folded.height > 20,
          `${id}：收合後仍看得到標題與展開鈕`, JSON.stringify(folded));
    if (folded.btnVisible) {
      await page.click(`#${id} .fold`);
    } else {
      // 按鈕看不到時不要去點它：Playwright 會等到逾時，整支測試就死在這裡，
      // 後面幾節一項都跑不到——得到的是一個例外，而不是一份完整的失敗清單。
      await page.evaluate(sid => document.getElementById(sid).classList.remove("collapsed"), id);
    }
    await page.waitForTimeout(110);
    const opened = await page.evaluate(sid => {
      const sec = document.getElementById(sid);
      return { open: !sec.classList.contains("collapsed"),
               height: Math.round(sec.getBoundingClientRect().height) };
    }, id);
    check(opened.open && opened.height > 60, `${id}：可以重新展開`, JSON.stringify(opened));
  }

  // 全部收合／展開
  await page.click('#toc button:has-text("全部收合")');
  await page.waitForTimeout(250);
  check(await page.$$eval("section.collapsed", n => n.length) === ids.length, "全部收合");
  check(await page.$$eval("section[id] .fold", ns => ns.every(f => f.offsetParent !== null)),
        "全部收合之後，每一節的展開鈕都還看得到");
  await page.evaluate(() =>
    document.querySelectorAll("section.collapsed").forEach(s => s.classList.remove("collapsed")));
  await page.click('#toc button:has-text("全部展開")');
  await page.waitForTimeout(250);
  check(await page.$$eval("section.collapsed", n => n.length) === 0, "全部展開");

  // 收合狀態要留到下次開啟；而點目錄連到收合中的區塊要自動展開
  await page.click("#trust .fold");
  await page.waitForTimeout(150);
  await page.reload({ waitUntil: "networkidle" });
  await page.waitForTimeout(2000);
  check(await page.evaluate(() => document.getElementById("trust").classList.contains("collapsed")),
        "重新整理之後，收合狀態還記得");
  await page.click('#toc a[data-for="trust"]');
  await page.waitForTimeout(300);
  check(await page.evaluate(() => !document.getElementById("trust").classList.contains("collapsed")),
        "點目錄連到收合中的區塊會自動展開（否則對方只看到一行標題，像壞掉）");

  // 錨點與捲動高亮
  await page.click('#toc a[data-for="seals"]');
  await page.waitForTimeout(800);
  check((await page.url()).endsWith("#seals"), "點目錄會把錨點寫進網址");
  check((await page.$$eval("#toc a.on", ns => ns.map(n => n.dataset.for)))[0] === "seals",
        "捲動高亮跟著目前的區塊");

  // 長表格截斷
  const capped = await page.evaluate(() => {
    const tb = document.getElementById("tbl-wl");
    if (!tb) return null;
    const rows = [...tb.querySelectorAll("tr:not(.more-row)")];
    const hidden = rows.filter(r => r.style.display === "none").length;
    return { total: rows.length, hidden, hasBar: !!tb.querySelector("tr.more-row") };
  });
  if (capped && capped.total > 9) {
    check(capped.hidden > 0 && capped.hasBar, "過長的表格先只顯示前幾列", JSON.stringify(capped));
    await page.click("#tbl-wl .more");
    await page.waitForTimeout(150);
    check(await page.evaluate(() =>
      [...document.querySelectorAll("#tbl-wl tr:not(.more-row)")].every(r => r.style.display !== "none")),
      "點「顯示其餘」會展開全部");
  } else {
    console.log("SKIP  長表格截斷（這份資料的列數還不夠多）");
  }

  // 三個寬度都不可以橫向溢出。注意不能用 getBoundingClientRect 判斷：
  // 可橫向捲動的容器裡的元素照樣回報超出視窗的座標，那是誤報。
  for (const width of [390, 820, 1280]) {
    const vp = await browser.newPage({ viewport: { width, height: 900 } });
    await vp.goto(URL, { waitUntil: "networkidle" });
    await vp.waitForTimeout(1800);
    const scrolled = await vp.evaluate(() => {
      window.scrollTo(500, 0);
      const x = window.scrollX;
      window.scrollTo(0, 0);
      return x;
    });
    check(scrolled === 0, `${width}px 寬不會橫向溢出`, `實際捲了 ${scrolled}px`);
    await vp.close();
  }

  // ── 互動功能：每一項都是「在預設狀態看起來正常、換個狀態就出錯」的那種 ──
  const winIds = await page.$$eval('#seg-window input', ns => ns.map(n => n.id.replace("win-", "")));
  if (winIds.includes("24h")) {
    await page.click('label:has(#win-24h)');
    await page.waitForTimeout(3000);
  }

  // 閒置分攤：畫面宣稱「兩種做法總額完全相同」，那就驗它
  const totalNow = () => page.evaluate(() => document.getElementById("t-total").textContent);
  await page.click("label:has(#idle-sep)");
  await page.waitForTimeout(400);
  const sepTotal = await totalNow();
  const sepDetail = await page.evaluate(() =>
    [...document.querySelectorAll("#tbl-dept tr")].map(r => r.lastElementChild.textContent).join("|"));
  await page.click("label:has(#idle-shr)");
  await page.waitForTimeout(400);
  const shrTotal = await totalNow();
  const shrDetail = await page.evaluate(() =>
    [...document.querySelectorAll("#tbl-dept tr")].map(r => r.lastElementChild.textContent).join("|"));
  check(sepTotal === shrTotal, "閒置獨立列帳 vs 按比例分攤：總額相同（畫面就是這樣宣稱的）",
        `${sepTotal} vs ${shrTotal}`);
  check(sepDetail !== shrDetail, "但各部門的金額要跟著變", sepDetail);
  await page.click("label:has(#idle-sep)");
  await page.waitForTimeout(300);

  // 部門篩選不可以影響「全叢集」的判斷
  const clusterWide = () => page.evaluate(() => ({
    total: document.getElementById("t-total").textContent,
    gate: document.getElementById("g-tag").textContent,
    unalloc: document.getElementById("t-unalloc").textContent,
  }));
  const before = await clusterWide();
  const deptIds = await page.$$eval("#seg-dept input", ns => ns.map(n => n.id));
  if (deptIds.length > 1) {
    await page.click(`label:has(#${deptIds[1]})`);
    await page.waitForTimeout(500);
    const after = await clusterWide();
    check(JSON.stringify(before) === JSON.stringify(after),
          "選單一部門時，總成本／出帳結論／無法分攤仍是全叢集的數字",
          JSON.stringify(before) + " vs " + JSON.stringify(after));
    const title = await page.evaluate(() => document.getElementById("t-dept-k").textContent);
    check(!title.includes("個部門合計"),
          "選單一部門時，卡片標題要跟著變（不可以還寫「N 個部門合計」）", title);
    await page.click("label:has(#dept-all)");
    await page.waitForTimeout(400);
  }

  // 短樣本的月推估要標成不可靠——數字沒算錯，但它跟可靠的數字長得一樣
  if (winIds.includes("1h")) {
    await page.click("label:has(#win-1h)");
    await page.waitForTimeout(3000);
    const thin = await page.evaluate(() => ({
      minutes: parseFloat(document.getElementById("m-minutes").textContent),
      warn: document.getElementById("m-thin").textContent.trim(),
      muted: document.body.classList.contains("thin-sample"),
    }));
    if (thin.minutes < 120) {
      check(thin.warn.length > 0 && thin.muted,
            `樣本只有 ${thin.minutes} 分鐘時，月推估要標成不可靠`, JSON.stringify(thin));
    } else {
      console.log(`SKIP  短樣本警告（這次的 1h 區間有 ${thin.minutes} 分鐘，不算短）`);
    }
    await page.click("label:has(#win-24h)");
    await page.waitForTimeout(3000);
  }

  check(errors.length === 0, "沒有 JavaScript 錯誤", errors.join(" | "));
  check(warnings.length === 0, "沒有觸發收合的安全網（有的話代表版面結構壞了）", warnings.join(" | "));

  await browser.close();
  const passed = results.filter(Boolean).length;
  console.log(`\n${passed}/${results.length} 通過`);
  process.exit(passed === results.length ? 0 : 1);
})();
