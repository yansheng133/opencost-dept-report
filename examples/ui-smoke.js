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
  const hiddenFolds = await page.$$eval("section[id] .fold", ns => ns
    .filter(f => f.offsetParent === null)
    .map(f => (f.closest("section[id]") || {}).id + "/" + (f.id || f.textContent.trim())));
  check(hiddenFolds.length === 0,
        "全部收合之後，每一節的展開鈕都還看得到（.fold 這個 class 只能給收合鈕用）",
        JSON.stringify(hiddenFolds));
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

  // ── 使用率折線圖 ──
  // 先挑資料點最多的那個區間再測。環境有斷層的時候，預設區間可能只剩兩三個點，
  // 折線相關的檢查會整組跳過——**一個會自動跳過的檢查，等於隨時可能永遠不跑。**
  const best = { win: null, n: -1 };
  for (const w of winIds) {
    await page.click(`label:has(#win-${w})`);
    await page.waitForTimeout(3000);
    // 注意：頁面裡的 DATA 是 let 宣告的，**不是 window 的屬性**（那是 var 才有的行為）。
    // 寫成 window.DATA 會永遠拿到 undefined，於是這個檢查在空資料上安靜地通過。
    const n = await page.evaluate(() =>
      (typeof DATA !== "undefined" && DATA.series
        ? DATA.series.points.filter(p => p.depts).length : 0));
    if (n > best.n) { best.n = n; best.win = w; }
  }
  if (best.win) {
    console.log(`INFO  折線圖用 ${best.win} 區間測（${best.n} 個資料點，是三個區間裡最多的）`);
    await page.click(`label:has(#win-${best.win})`);
    await page.waitForTimeout(3000);
  }

  const chart = await page.evaluate(() => {
    const svg = document.getElementById("ln-svg");
    if (!svg) return null;
    const paths = [...svg.querySelectorAll("path.ln-path")];
    const ends = [...svg.querySelectorAll("text.ln-end")].map(t => +t.getAttribute("y")).sort((a, b) => a - b);
    const gaps = ends.slice(1).map((y, i) => y - ends[i]);
    return { lines: paths.length, hasCapture: !!svg.querySelector("rect[fill='transparent']"),
             labels: ends.length, minGap: gaps.length ? Math.min(...gaps) : 99,
             maxLabelY: ends.length ? ends[ends.length - 1] : 0 };
  });
  if (chart) {
    check(chart.lines > 0, "折線圖有畫出線", JSON.stringify(chart));
    check(chart.minGap >= 13, "線末端的直接標示沒有互相重疊", JSON.stringify(chart));
    // 標籤掉進 X 軸刻度那一列的話會跟時間字重疊（viewBox 高 260、底部留白 26）
    check(chart.maxLabelY <= 240, "直接標示沒有掉進 X 軸刻度那一列", JSON.stringify(chart));
    // SVG 只在有畫東西的地方收得到指標事件，沒有這塊透明矩形，滑過空白處不會有反應
    check(chart.hasCapture, "有捕捉滑鼠用的透明矩形");

    // 時間軸必須照真實時間，不是點的序號：否則 6 小時的間隔跟 3 天的間隔看起來一樣
    const axis = await page.evaluate(() => {
      const S = DATA.series;
      if (!S) return null;
      const lead = S.points.findIndex(p => p.depts);
      if (lead <= 0) return { skip: true };
      const firstX = Math.min(...[...document.querySelectorAll("#ln-svg path.ln-path")]
        .map(p => parseFloat(p.getAttribute("d").match(/M([\d.]+)/)[1])));
      return { lead, firstX };
    });
    if (axis && !axis.skip) {
      check(axis.firstX > 60,
            `序列前面有 ${axis.lead} 段空白，第一個資料點要往右縮（x=${Math.round(axis.firstX)}）`,
            JSON.stringify(axis));
    } else {
      console.log("SKIP  時間軸留白（這次的序列開頭沒有空白段）");
    }

    // 中間挖一個洞，折線要斷開。一段連續資料畫一個 <path>，所以斷開＝路徑數變多。
    const broke = await page.evaluate(() => {
      const before = document.querySelectorAll("#ln-svg path.ln-path").length;
      const withData = DATA.series.points.filter(p => p.depts);
      // 要 5 個點以上才測得準：只有 3 個點時，挖掉中間那個會讓兩邊各剩一個，
      // 而單點是畫成圓點不是路徑——路徑數反而變少，看起來像「沒有斷開」。
      if (withData.length < 5) return null;
      const mid = DATA.series.points.indexOf(withData[Math.floor(withData.length / 2)]);
      const keep = DATA.series.points[mid].depts;
      DATA.series.points[mid].depts = null;
      renderUsage();
      const after = document.querySelectorAll("#ln-svg path.ln-path").length;
      DATA.series.points[mid].depts = keep;
      renderUsage();
      return { before, after };
    });
    if (!broke) {
      console.log("SKIP  折線斷開（這次的序列不到 5 個資料點，測不準）");
    } else {
      check(broke.after > broke.before,
            "資料缺一段時折線會斷開，不會內插（把兩端連起來等於偽造那段量測）",
            JSON.stringify(broke));
    }
  } else {
    console.log("SKIP  使用率折線圖（頁面上沒有這個區塊）");
  }

  // ── 月報：來源是封存，不是即時查詢 ──
  const months = await page.evaluate(() => {
    const cards = [...document.querySelectorAll("#months .mon")];
    if (!cards.length) return { none: true, text: document.getElementById("months").innerText };
    return {
      count: cards.length,
      first: cards[0].innerText,
      data: (typeof MONTHS !== "undefined" ? MONTHS : []).map(
        m => ({ month: m.month, verdict: m.verdict,
                sealed: m.sealedDays, missing: (m.missingDays || []).length })),
    };
  });
  if (months.none) {
    check(/還沒有任何封存/.test(months.text || ""), "沒有封存時，月報要說明原因而不是空白", months.text);
  } else {
    check(months.count > 0, "月報有卡片", JSON.stringify(months.data));
    // 缺日子的月份**不可以**顯示成可出帳——N 天的加總不是一個月的帳單
    check((months.data || []).length > 0, "讀得到月報的資料（空陣列會讓下面的檢查無聲通過）",
          JSON.stringify(months.data));
    const bad = (months.data || []).filter(m => m.missing > 0 && m.verdict === "ok");
    check(bad.length === 0, "缺日子的月份不可以判定為可出帳", JSON.stringify(bad));
    // 缺漏必須在畫面上講出來，不能只是數字小一點
    const withMissing = (months.data || []).find(m => m.missing > 0);
    if (withMissing) {
      check(/沒有封存的日子/.test(months.first), "缺漏的日子要在卡片上列出來", months.first.slice(0, 120));
    }
  }

  // ── 欄位說明 ──
  const hints = await page.evaluate(() => {
    const all = [...document.querySelectorAll(".hint")];
    // 數字欄位光看名字看不出是什麼，這幾個是最容易被誤讀的
    const must = ["CPU 核時", "月推估", "覆蓋率", "小計", "完整性"];
    const texts = all.map(n => n.textContent.trim());
    return {
      count: all.length,
      missing: must.filter(m => !texts.includes(m)),
      focusable: all.every(n => n.tabIndex === 0),
      // 說明要掛在標籤上，不是掛在數字上——大數字底下一條虛線很怪
      onValues: ["t-total", "t-idle", "t-unalloc"]
        .filter(id => (document.getElementById(id) || {}).classList?.contains("hint")),
    };
  });
  check(hints.count > 20, `有說明的欄位有 ${hints.count} 個`);
  check(hints.missing.length === 0, "最容易被誤讀的欄位都有說明", JSON.stringify(hints.missing));
  check(hints.focusable, "說明可以用鍵盤聚焦（只綁 hover 的話，觸控與鍵盤使用者永遠看不到）");
  check(hints.onValues.length === 0, "說明掛在標籤上，不是掛在數字上", JSON.stringify(hints.onValues));

  // 滑過去真的要出現，而且離開要收掉
  const shown = await page.evaluate(async () => {
    const el = [...document.querySelectorAll(".hint")].find(n => n.textContent.trim() === "月推估");
    if (!el) return null;
    el.scrollIntoView({ block: "center" });
    await new Promise(r => setTimeout(r, 200));
    el.dispatchEvent(new PointerEvent("pointerenter", { bubbles: true }));
    await new Promise(r => setTimeout(r, 150));
    const on = { op: getComputedStyle(document.getElementById("tip")).opacity,
                 len: document.getElementById("tip").textContent.length };
    el.dispatchEvent(new PointerEvent("pointerleave", { bubbles: true }));
    await new Promise(r => setTimeout(r, 150));
    return { on, offOpacity: getComputedStyle(document.getElementById("tip")).opacity };
  });
  if (shown) {
    check(shown.on.op === "1" && shown.on.len > 10, "滑過欄位名稱會出現說明", JSON.stringify(shown));
    check(shown.offOpacity === "0", "移開之後說明會收掉", JSON.stringify(shown));
  }

  // ── 指定日期：封存之外唯一能回頭看某一天的方法 ──
  const dayPick = await page.$("#pick-day");
  if (dayPick) {
    const before = await page.evaluate(() => document.getElementById("t-total").textContent);
    const sealDay = await page.evaluate(() =>
      (typeof SEALS !== "undefined" && SEALS.length) ? SEALS[SEALS.length - 1].day : null);
    if (sealDay) {
      await page.fill("#pick-day", sealDay);
      await page.waitForFunction(d =>
        document.querySelector(".eyebrow").textContent.includes(d), sealDay, { timeout: 30000 });
      const after = await page.evaluate(() => ({
        eyebrow: document.querySelector(".eyebrow").textContent,
        win: document.getElementById("m-window").textContent,
        total: document.getElementById("t-total").textContent,
      }));
      check(after.eyebrow.includes(sealDay),
            `選日期會切到那一天（${sealDay}），而且標題列要講明白不是即時資料`, JSON.stringify(after));
      check(after.win.startsWith(sealDay), "期間確實是那一天", after.win);
      // 指定日期時折線圖也要有資料。加了日期查詢卻忘了教序列函式認絕對區間的話，
      // 圖會整個空掉——而那正是「回頭看某一天」最想看的東西。
      const daySeries = await page.evaluate(() =>
        (typeof DATA !== "undefined" && DATA.series)
          ? { covered: DATA.series.covered, expected: DATA.series.expected, step: DATA.series.step }
          : null);
      check(daySeries && daySeries.covered > 0,
            "指定某一天時，使用率折線圖也要有資料", JSON.stringify(daySeries));
      await page.click("#pick-clear");
      await page.waitForFunction(() =>
        !document.querySelector(".eyebrow").textContent.includes("檢視"), null, { timeout: 30000 });
      check(true, "可以回到即時檢視");
    } else {
      console.log("SKIP  指定日期（還沒有任何封存可以當測試對象）");
    }
  }

  // 覆蓋率的分母要是「要求的區間」，不是「OpenCost 回傳的區間」——
  // 用後者的話，資料在邊緣被截斷時會顯示 100%（實測有一天只有 60 分鐘卻報 100%）
  const cov = await page.evaluate(() =>
    (typeof DATA !== "undefined" && DATA.coverage) ? DATA.coverage : null);
  if (cov && cov.available !== false) {
    check(cov.requestedMinutes > 0, "覆蓋率有記錄「要求的區間長度」", JSON.stringify(cov));
    check(cov.measuredMinutes <= cov.requestedMinutes + 1,
          "實際涵蓋不會超過要求的區間", JSON.stringify(cov));
    const expect = Math.round(cov.scrapePct * cov.windowPct) / 100;
    check(Math.abs(cov.pct - expect) < 0.5,
          "實際涵蓋率 = 抓取覆蓋率 × 區間涵蓋率", JSON.stringify(cov));
  }

  // 時間軸不可以延伸到未來。OpenCost 會回一個「還沒到」的空格子，
  // 把它畫進去的話，6 小時的間隔會讓軸多伸出去半天，看過往區間時就覺得怪。
  const future = await page.evaluate(() => {
    if (typeof DATA === "undefined" || !DATA.series) return null;
    const now = Date.now();
    return DATA.series.points.filter(p => Date.parse(p.start) > now + 60000)
                             .map(p => p.start);
  });
  if (future) {
    check(future.length === 0, "折線圖的時間軸不會延伸到未來", JSON.stringify(future));
  }

  check(errors.length === 0, "沒有 JavaScript 錯誤", errors.join(" | "));
  check(warnings.length === 0, "沒有觸發收合的安全網（有的話代表版面結構壞了）", warnings.join(" | "));

  await browser.close();
  const passed = results.filter(Boolean).length;
  console.log(`\n${passed}/${results.length} 通過`);
  process.exit(passed === results.length ? 0 : 1);
})();
