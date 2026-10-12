// ricow Web UI 真机走查 (051, 承接 042 的教训)。
//
// ── 为什么需要它 ────────────────────────────────────────────────────────────────
// `e2e_web.py` 的断言压倒性地是"对 HTTP 取回的 JS/HTML 做子串匹配", 它证明的是
// **那段字符串确实被服务出去了**, 不是"这个功能在浏览器里能跑"。042 的 `S.onUndo` 漏导出
// 让打开策略页整片 `ReferenceError` 时, `node --check` / grep 扫描 / Rust 单测 / Python 冒烟
// **四道静态门全绿** —— 因为没有一道进得了浏览器运行期。
//
// 本脚本走 CDP 真正加载页面并操作 DOM, 补上这一层。它**零依赖**(Node 22 内建 `fetch` 与
// `WebSocket`), 进仓库、可复跑、可进 CI。
//
// ── 怎么跑 ─────────────────────────────────────────────────────────────────────
//   1) 起服务(常驻):
//      RICOW_ROOT=<repo>/tmp/verify-web ./target/debug/ricow web --no-open --port 8799
//   2) 起 headless 浏览器(Edge/Chrome 的 CDP 端口, 默认 9333):
//      msedge --headless=new --remote-debugging-port=9333 --user-data-dir=<tmp> about:blank
//   3) 跑本脚本:
//      RICOW_WEB_URL="http://127.0.0.1:8799/?token=<T>" node scripts/probe_web_ui.mjs
//      (端口非 9333 时加 RICOW_CDP_PORT=9334)
//
// 退出码: 0 = 全绿; 1 = 有 FAIL; 2 = 脚本自身异常(没连上浏览器 / 没给 URL)。
"use strict";

const CDP = "http://127.0.0.1:" + (process.env.RICOW_CDP_PORT || "9333");
const url = process.env.RICOW_WEB_URL || process.argv[2];
if (!url) {
  console.error("缺少目标地址: 设 RICOW_WEB_URL(或作为第一个参数传入)");
  process.exit(2);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let ws = null;
let seq = 0;
const pending = new Map();
const send = (method, params = {}) =>
  new Promise((res, rej) => {
    const id = ++seq;
    pending.set(id, { res, rej });
    ws.send(JSON.stringify({ id, method, params }));
  });

let fails = 0;
const check = (name, cond, detail) => {
  if (cond) console.log("  PASS  " + name);
  else {
    fails += 1;
    console.log("  FAIL  " + name + "   <-- " + (detail === undefined ? "" : detail));
  }
};

// 浏览器侧异常收集: 这是本脚本相对静态门的**核心增量** —— 一条未捕获的 ReferenceError
// 就足以让整片视图失效, 而任何 grep 都看不见它。
const pageErrors = [];
let errMark = 0;
// 浏览器在页面**未声明** `<link rel="icon">` 时会自动请求 `/favicon.ico`; 本服务所有端点都在
// token 门之后, 该请求必然 404 —— 这是**浏览器的固定行为, 不是产品路径**。
// 单独计数并把原因打出来(不静默吞掉), 免得"零异常"这条断言被一条噪声永久染红而失去意义。
let faviconNoise = 0;

async function ev(expr) {
  const r = await send("Runtime.evaluate", {
    expression: expr,
    returnByValue: true,
    awaitPromise: true,
  });
  if (r.exceptionDetails) {
    return { __threw: r.exceptionDetails.exception?.description || "evaluate 抛错" };
  }
  return r.result.value;
}

async function key(k, code, vk, modifiers = 0) {
  for (const type of ["keyDown", "keyUp"]) {
    await send("Input.dispatchKeyEvent", {
      type,
      modifiers,
      key: k,
      code,
      windowsVirtualKeyCode: vk,
      nativeVirtualKeyCode: vk,
    });
  }
}

/// 断言"从上次标记以来没有新的页面异常"(逐视图调用, 便于定位是谁炸的)。
function checkNoErrors(label) {
  const fresh = pageErrors.slice(errMark);
  check(label + " 无未捕获异常", fresh.length === 0, fresh.slice(0, 3).join(" | "));
  errMark = pageErrors.length;
}

// 六视图: 与 router.js 的 VIEW_NAMES 同口径。
const VIEWS = ["chat", "markets", "strategies", "runs", "keys", "settings"];

async function main() {
  // 每次开一个**全新标签页**, 不复用浏览器里已存在的那一个。
  // 原因: `Log.enable` 会把该页此前的 console 条目**重放**一遍 —— 复用旧页会把上一轮跑出的
  // 报错当成这一轮页面加载期的错(实测: 上一轮 ⑧ 段故意造的 409 会漂到这一轮的"零异常"断言上)。
  // 新页则没有任何历史可重放, "全程零未捕获异常" 这条才真的只盯本轮。
  let page = null;
  for (let i = 0; i < 40 && !page; i++) {
    try {
      const t = await (await fetch(CDP + "/json/new?about:blank", { method: "PUT" })).json();
      if (t && t.webSocketDebuggerUrl) page = t;
    } catch (_) {}
    if (!page) await sleep(250);
  }
  if (!page) {
    // 老版本 Chromium 不接受 `PUT /json/new`: 退回复用现有页(此时需自己容忍上轮残响)。
    try {
      const list = await (await fetch(CDP + "/json")).json();
      page = list.find((t) => t.type === "page");
    } catch (_) {}
  }
  if (!page) {
    console.error("连不上 CDP(" + CDP + "), 确认 headless 浏览器已用 --remote-debugging-port 启动");
    process.exit(2);
  }
  ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  ws.addEventListener("message", (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result);
      return;
    }
    if (m.method === "Runtime.exceptionThrown") {
      pageErrors.push(
        m.params.exceptionDetails?.exception?.description ||
          m.params.exceptionDetails?.text ||
          "未知异常"
      );
    } else if (m.method === "Log.entryAdded" && m.params.entry.level === "error") {
      const text = m.params.entry.text || "";
      if (/favicon/i.test(text) || /favicon/i.test(m.params.entry.url || "")) {
        faviconNoise += 1;
        return;
      }
      pageErrors.push("[console.error] " + text);
    }
  });

  await send("Runtime.enable");
  await send("Log.enable");
  await send("Page.enable");
  await send("Page.navigate", { url });
  await sleep(6000);

  console.log("=".repeat(78));
  console.log("ricow Web UI · CDP 真机走查");
  console.log("=".repeat(78));

  // ---- ① token 载体(B2/043 回归): 页面渲染后 R.TOKEN 必须非空 ----
  const token = await ev("window.Ricow && window.Ricow.TOKEN");
  check("R.TOKEN 非空(meta 载体兜底生效)", typeof token === "string" && token.length > 0, String(token));

  // ---- ①' 格式化出口的边界(051 B2/B3): 全站唯一出口必须"绝不吐坏值" ----
  // 这几条只有真机跑得出来 —— 它们是 `R.fmt` 的**运行时**行为, 任何 grep 都看不见。
  const numSmall = await ev("window.Ricow.fmt.num(1e-7)");
  check(
    "fmt.num(1e-7) 不泄漏科学计数法(B2)",
    numSmall === "0.0000001",
    String(numSmall)
  );
  check("fmt.num 去尾零", (await ev("window.Ricow.fmt.num(0.30000000000000004)")) === "0.3");
  const numNull = await ev('window.Ricow.fmt.num(null)');
  check("fmt.num(null) → 破折号", numNull === "—", String(numNull));
  const tHuge = await ev("window.Ricow.fmt.time(1e300)");
  check("fmt.time(1e300) 不是字面量 Invalid Date(B3)", tHuge === "—", String(tHuge));
  const tSecBad = await ev('window.Ricow.fmt.timeSec("abc")');
  check("fmt.timeSec('abc') → 空串而非 Invalid Date(B3)", tSecBad === "", JSON.stringify(tSecBad));
  check(
    "fmt.timeSec(0) 仍按缺值处理(语义未变)",
    (await ev("window.Ricow.fmt.timeSec(0)")) === ""
  );

  // ---- ①'' 术语装饰不会死循环(051 B6) ----
  // 造一个含术语名的节点 → decorate → 必须**返回**且真的包成了 `.term` 按钮。
  // 零长匹配会让这个 evaluate 永不返回, 于是 CDP 调用超时 —— 这本身就是失败信号。
  const decorate = await ev(`(() => {
    const box = document.createElement("div");
    box.id = "probe-decorate";
    box.textContent = "最大回撤与手续费";
    document.body.appendChild(box);
    window.Ricow.glossary.decorate(box);
    const n = box.querySelectorAll(".term").length;
    box.remove();
    return n;
  })()`);
  check("术语装饰能返回并包出词条(B6 零长匹配保护)", Number(decorate) >= 1, String(decorate));
  checkNoErrors("格式化边界与术语装饰");

  // ---- ② 六个视图逐个激活: 每个都要真"亮"且不抛异常 ----
  for (const v of VIEWS) {
    await ev(`location.hash = "#${v}"`);
    await sleep(500);
    const active = await ev(
      `(() => { const s = document.getElementById("view-${v}"); return s ? (s.classList.contains("active") && !s.hidden) : null; })()`
    );
    check("视图 #" + v + " 激活", active === true, String(active));
    checkNoErrors("视图 #" + v);
  }

  // ---- ③ 非法 hash 回退: 不但要回 #chat, 且唯一真相 R.currentView 必须跟着校正(B12) ----
  await ev('location.hash = "#nope"');
  await sleep(500);
  const cur = await ev("window.Ricow.currentView");
  const hash = await ev("location.hash");
  check("非法 hash 回退到 #chat", hash === "#chat", hash);
  check("R.currentView 随校正更新(B12)", cur === "chat", String(cur));
  checkNoErrors("非法 hash 回退");

  // ---- ④ 帮助抽屉: 打开后要有内容、且不吃异常 ----
  await ev('location.hash = "#strategies"');
  await sleep(400);
  await ev("window.Ricow.help.open()");
  await sleep(400);
  const helpOpen = await ev(
    '(() => { const d = document.getElementById("help-drawer"); return d ? !d.hidden : null; })()'
  );
  check("帮助抽屉可打开", helpOpen === true, String(helpOpen));
  const helpHasSections = await ev(
    'document.querySelectorAll("#help-drawer .help-sec, #help-drawer section").length'
  );
  check("帮助抽屉有分段内容", Number(helpHasSections) > 0, String(helpHasSections));
  checkNoErrors("帮助抽屉");
  await ev("window.Ricow.help.close()");
  await sleep(200);

  // ---- ⑤ 命令面板: Ctrl+K 开 → 过滤 → Esc 关(键盘链路) ----
  await key("k", "KeyK", 75, 2);
  await sleep(300);
  const palOpen = await ev("document.getElementById('palette').hidden === false");
  check("Ctrl+K 打开命令面板", palOpen === true, String(palOpen));
  await send("Input.insertText", { text: "回撤" });
  await sleep(300);
  const hits = await ev(
    "Array.from(document.querySelectorAll('#palette-list .palette-item-label')).map(n => n.textContent)"
  );
  check("面板能按术语过滤", Array.isArray(hits) && hits.includes("最大回撤"), JSON.stringify(hits));
  checkNoErrors("命令面板");
  await key("Escape", "Escape", 27);
  await sleep(200);
  const palClosed = await ev("document.getElementById('palette').hidden === true");
  check("Esc 关闭命令面板", palClosed === true, String(palClosed));

  // ---- ⑥ 窄屏抽屉: 三种收起路径都要真的收起(B7 的行为断言) ----
  await send("Emulation.setDeviceMetricsOverride", {
    width: 480,
    height: 900,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await sleep(300);
  const openDrawer = async () => {
    await ev("document.getElementById('side-toggle').click()");
    await sleep(250);
    return ev("document.getElementById('app').classList.contains('side-open')");
  };
  check("窄屏点 ☰ 打开抽屉", (await openDrawer()) === true);
  await ev("document.getElementById('sidebar').click()");
  await sleep(250);
  check(
    "点侧栏(含会话列表)收起抽屉(B7)",
    (await ev("document.getElementById('app').classList.contains('side-open')")) === false
  );
  check("窄屏点 ☰ 二次打开", (await openDrawer()) === true);
  await ev("document.getElementById('nav').querySelector('a').click()");
  await sleep(300);
  check(
    "点导航项收起抽屉(B7)",
    (await ev("document.getElementById('app').classList.contains('side-open')")) === false
  );
  check("窄屏第三次打开", (await openDrawer()) === true);
  await ev('location.hash = "#runs"'); // 程序化导航: 这条只靠 hashchange 兜
  await sleep(400);
  check(
    "程序化导航收起抽屉(B7)",
    (await ev("document.getElementById('app').classList.contains('side-open')")) === false
  );
  checkNoErrors("窄屏抽屉");
  await send("Emulation.clearDeviceMetricsOverride");
  await sleep(200);

  // ---- ⑦ 主题切换: <html data-theme> 真的跟着变 ----
  await ev(
    '(() => { const s = document.getElementById("theme-select"); s.value = "light"; s.dispatchEvent(new Event("change", { bubbles: true })); })()'
  );
  await sleep(400);
  const themeLight = await ev("document.documentElement.dataset.theme");
  check("切主题后 data-theme 生效", themeLight === "light", String(themeLight));
  await ev(
    '(() => { const s = document.getElementById("theme-select"); s.value = "dark"; s.dispatchEvent(new Event("change", { bubbles: true })); })()'
  );
  await sleep(400);
  check("可切回 dark", (await ev("document.documentElement.dataset.theme")) === "dark");
  checkNoErrors("主题切换");

  // ---- ⑧ 052: 策略配置断层修复(AI 卡模式 chip / 空参数表单引导卡) ----
  //
  // 这一节盯的正是静态门**抓不到**的那一类: 模式 chip 与引导卡都是**动态建 DOM + 动态文案**,
  // 少一个 `S.xxx` 导出、`data-zh` 没跟着模式同步、或 `renderParams` 把引导卡一起清掉 ——
  // 语法检查 / grep / Rust 单测 / HTTP 冒烟四道门全绿, 只有真浏览器会露馅。
  await ev('location.hash = "#strategies"');
  await sleep(500);

  // 先造一个"没有参数清单"的用户策略(裸 .lua) —— 这就是"复制完只剩一个交易对框"的那个状态。
  // 名字带时间戳: 策略没有删除端点, 固定名在第二次跑就会 409, 而前端 R.api 会顺手 console.error,
  // 把下面两条"无未捕获异常"的收口一起染红 —— 那是**脚本自身不幂等**, 不是产品缺陷。
  const bareName = await ev("'zz_cfg_probe' + String(Date.now() % 100000000)");
  await ev(
    "window.Ricow.api('/api/strategies', { method: 'POST', body: { name: " +
      JSON.stringify(bareName) +
      ", market: 'spot'," +
      " pair: 'ETHUSDT', params: {}, code: 'local ctx = ...\\nfunction on_tick(ctx)\\n" +
      "  local sp = ctx:config_f64(\"spacing_pct\")\\n  if sp > 0 then ctx:log(\"t\") end\\nend\\n' } })" +
      ".then(function () { return 'ok'; }).catch(function (e) { return 'ERR:' + (e && e.code); })"
  );
  await ev("location.hash = '#strategies/' + " + JSON.stringify(bareName));
  await sleep(1000);

  const guideVisible = await ev(
    "(() => { const g = document.getElementById('sg-params-guide'); return g ? !g.hidden : null; })()"
  );
  check("裸脚本详情页出现参数引导卡(052)", guideVisible === true, String(guideVisible));
  const guideBtns = await ev(
    "(() => Array.from(document.querySelectorAll('#sg-params-guide .sg-guide-actions button'))" +
      ".map(function (b) { return b.textContent; }))()"
  );
  check(
    "引导卡有两个动作(识别参数 / 问 AI)",
    Array.isArray(guideBtns) && guideBtns.length === 2,
    JSON.stringify(guideBtns)
  );
  const paramRows = await ev("document.querySelectorAll('#sg-params .sg-param').length");
  check("清单为空时参数表单真的只剩交易对一个框(复现原症状)", paramRows === 1, String(paramRows));

  // 「问 AI 怎么配」在清单还为空时必须**只引导** —— 后端 `mode=params` 清单为空本就是 400,
  // 假装能直接问等于把用户送去撞墙。
  const askAiMsg = await ev(
    "(() => { const bs = document.querySelectorAll('#sg-params-guide .sg-guide-actions button');" +
      " if (bs.length < 2) return null; bs[1].click();" +
      " const m = document.getElementById('sg-params-guide-msg'); return m ? m.textContent : null; })()"
  );
  check(
    "清单为空时点「问 AI 怎么配」给出下一步指引(不是空动作)",
    typeof askAiMsg === "string" && askAiMsg.length > 0,
    String(askAiMsg).slice(0, 50)
  );

  // AI 卡默认停在「调参数」: 用户的困惑正是"不知道该配什么", 先给一条**不动代码**的路。
  const chipDefault = await ev(
    "(() => { const p = document.getElementById('sg-ai-mode-params');" +
      " const c = document.getElementById('sg-ai-mode-code');" +
      " return p && c ? [p.classList.contains('on'), c.classList.contains('on')] : null; })()"
  );
  check(
    "AI 卡默认停在「调参数」chip(052)",
    Array.isArray(chipDefault) && chipDefault[0] === true && chipDefault[1] === false,
    JSON.stringify(chipDefault)
  );
  const goDefault = await ev("document.getElementById('sg-ai-go').textContent");
  check("执行按钮文案 = 按意图调参(N3)", goDefault === "按意图调参", String(goDefault));
  const hintDefault = await ev("document.getElementById('sg-ai-mode-hint').textContent");
  check(
    "模式提示语是「调参数」那一段(含「不改代码」)",
    typeof hintDefault === "string" && hintDefault.indexOf("不改代码") >= 0,
    String(hintDefault).slice(0, 60)
  );

  // 切到「改代码逻辑」: chip 高亮 + 提示语**同时**换 —— 少一处就是"看着切了、语义还是旧的"。
  await ev("document.getElementById('sg-ai-mode-code').click()");
  await sleep(250);
  const chipCode = await ev("document.getElementById('sg-ai-mode-code').classList.contains('on')");
  const hintCode = await ev("document.getElementById('sg-ai-mode-hint').textContent");
  check(
    "切到「改代码逻辑」后 chip 高亮 + 提示语换成改代码那段",
    chipCode === true && typeof hintCode === "string" && hintCode.indexOf("Lua") >= 0,
    chipCode + " / " + String(hintCode).slice(0, 50)
  );

  // 切语言: chip 的静态标签走 `data-zh`, 而**执行按钮与提示语随模式变** —— 若只改 textContent
  // 不同步 `data-*`, 这一步就会被旧模式的文案盖回去(052 特意防的就是这条)。当前模式 = 改代码。
  await ev("window.Ricow.applyLang('en')");
  await sleep(400);
  const goEn = await ev("document.getElementById('sg-ai-go').textContent");
  const hintEn = await ev("document.getElementById('sg-ai-mode-hint').textContent");
  check(
    "切英文后执行按钮仍是「当前模式」的英文(没被旧模式盖回)",
    goEn === "Change code",
    String(goEn)
  );
  check(
    "切英文后模式提示语也换成英文",
    typeof hintEn === "string" && hintEn.indexOf("The AI rewrites the Lua") >= 0,
    String(hintEn).slice(0, 60)
  );
  await ev("window.Ricow.applyLang('zh')");
  await sleep(400);
  const goZh = await ev("document.getElementById('sg-ai-go').textContent");
  check("切回中文后文案复原", goZh === "改代码逻辑", String(goZh));

  // ---- N3: 「调整参数」改成多轮对话 + 自动评估之后的界面面 ----
  // 占位文字从"共用一句"改成**按模式分开**: 参数模式要说的是"想要的效果"(用户本来就不该被迫
  // 说出参数名), 代码模式说的才是"策略逻辑怎么改"。共用一句必然误导其中一个模式, 所以这里
  // 逐模式逐语言钉住它 —— 包括"切语言后仍停在当前模式那句"(只改 textContent 不同步 data 属性
  // 就会被初始模式那句盖回来, 与按钮文案是同一个坑)。
  await ev("document.getElementById('sg-ai-mode-params').click()");
  await sleep(200);
  const phParams = await ev("document.getElementById('sg-ai-instr').placeholder");
  check(
    "N3 参数模式占位文字是「说效果」那句(不逼用户说参数名)",
    typeof phParams === "string" && phParams.indexOf("想要的效果") >= 0,
    String(phParams).slice(0, 50)
  );
  await ev("document.getElementById('sg-ai-mode-code').click()");
  await sleep(200);
  const phCode = await ev("document.getElementById('sg-ai-instr').placeholder");
  check(
    "N3 切到「改代码逻辑」后占位文字跟着换",
    typeof phCode === "string" && phCode.indexOf("策略逻辑") >= 0,
    String(phCode).slice(0, 50)
  );
  await ev("window.Ricow.applyLang('en')");
  await sleep(400);
  const phEn = await ev("document.getElementById('sg-ai-instr').placeholder");
  check(
    "N3 切英文后占位文字是**当前模式**的英文(没被初始模式盖回)",
    typeof phEn === "string" && phEn.indexOf("strategy logic") >= 0,
    String(phEn).slice(0, 60)
  );
  await ev("window.Ricow.applyLang('zh')");
  await sleep(400);
  await ev("document.getElementById('sg-ai-mode-params').click()");
  await sleep(200);

  // 线程与评估面板的初始形态: 空对话收起 / 没改动就不摆评估按钮。
  // 后者尤其重要 —— 一个点了必然 400 的按钮比不摆更糟。
  const threadInit = await ev(
    "(() => { const t = document.getElementById('sg-ai-thread'); return t ? t.hidden : null; })()"
  );
  check("N3 空对话时线程容器收起(hidden)", threadInit === true, String(threadInit));
  const assessInit = await ev(
    "(() => { const a = document.getElementById('sg-ai-assess'); return a ? a.hidden : null; })()"
  );
  check("N3 没有改动时不显示影响评估面板", assessInit === true, String(assessInit));
  // 清空对话按钮在**空线程**上点一下也不能报错(它内部会重建线程与评估面板)。
  const clearOk = await ev(
    "(() => { const b = document.getElementById('sg-ai-clear');" +
      " if (!b) return 'missing'; b.click(); return 'ok'; })()"
  );
  check("N3 清空对话按钮可点(空线程点它也不炸)", clearOk === "ok", String(clearOk));
  const threadAfterClear = await ev(
    "(() => { const t = document.getElementById('sg-ai-thread'); return t ? t.hidden : null; })()"
  );
  check("N3 清空后线程仍收起", threadAfterClear === true, String(threadAfterClear));

  // N2-C 复核(真机抓到才成立的判据): 「让 AI 起中文名」按钮与清单卡的「策略中文名」输入框
  // 曾经**撞 id**(两个都叫 `sg-mf-name`), 文案键也**同名**(都叫 `sgMfName`) —— 于是
  // `getElementById` 只认先渲染的输入框(按钮上的 click 挂到了文本框)、按钮显示成「策略中文名」。
  // 这四条静态门一条都抓不到: 语法合法、双语齐、引用有定义 —— **值**是错的。只有真点一下才知道。
  const nameSplit = await ev(
    "(() => { const b = document.getElementById('sg-mf-autoname');" +
      " const i = document.getElementById('sg-mf-name');" +
      " return [!!b, !!(i && i.tagName === 'INPUT'), b ? b.textContent : null," +
      " i ? i.value : null]; })()"
  );
  check(
    "N2 起中文名按钮与策略名输入框 id 不撞(按钮独立存在)",
    Array.isArray(nameSplit) && nameSplit[0] === true && nameSplit[1] === true,
    JSON.stringify(nameSplit)
  );
  check(
    "N2 起中文名按钮文案是「让 AI 起中文名」(没被同名键盖成「策略中文名」)",
    Array.isArray(nameSplit) && nameSplit[2] === "让 AI 起中文名",
    String(nameSplit && nameSplit[2])
  );
  // 反向对照: 文本框里装的是清单里的**策略名**(一个 input 该有的样子), 不是被按钮抢走了事件。
  check(
    "N2 策略名输入框仍是可编辑文本框(没被按钮顶掉)",
    Array.isArray(nameSplit) && typeof nameSplit[3] === "string",
    JSON.stringify(nameSplit)
  );

  // 引导卡的「从脚本识别参数」真点一次: 必须从编辑器里的代码认出 `spacing_pct` 并把清单行填好。
  const guideClicked = await ev(
    "(() => { const b = document.querySelector('#sg-params-guide .sg-guide-actions button');" +
      " if (!b) return false; b.click(); return true; })()"
  );
  check("引导卡第一个动作可点(从脚本识别参数)", guideClicked === true);
  await sleep(1200);
  const mfKeys = await ev(
    "(() => Array.from(document.querySelectorAll('#sg-mf-params .sg-mf-row .sg-mf-k'))" +
      ".map(function (k) { return k.value; }))()"
  );
  check(
    "识别后清单编辑器真的多出这个参数行(spacing_pct)",
    Array.isArray(mfKeys) && mfKeys.indexOf("spacing_pct") >= 0,
    JSON.stringify(mfKeys)
  );
  const mfTy = await ev(
    "(() => { const r = document.querySelector('#sg-mf-params .sg-mf-row');" +
      " const t = r && r.querySelector('.sg-mf-ty'); return t ? t.value : null; })()"
  );
  check("识别出的类型按 API 语义填好(f64)", mfTy === "f64", String(mfTy));

  // ---- ⑨ 052 用户原始路径: 列表里「复制」一个内置 → 副本页参数表是满的 ----
  //
  // 这一条直接复刻用户报的场景("通过 Web UI 复制新建一个策略…完全不知道该怎么配置")。
  // 走的是真 DOM: 找行 → 点复制 → 弹窗填名 → 提交 → 落到副本详情页看参数表。
  await ev('location.hash = "#strategies"');
  await sleep(700);
  // 名字带时间戳, 免得重跑撞 409(没有删除端点, 只能各跑各的)。
  const copyName = await ev("'zzcopy' + String(Date.now() % 100000000)");
  // 内置显示名从 API 现取, 不硬编码中文名 —— 清单改名不会把这条变成假红。
  const pairName = await ev(
    "window.Ricow.api('/api/strategies').then(function (rs) {" +
      " var r = rs.filter(function (x) { return x.id === 'paired_grid'; })[0];" +
      " return r ? r.name : null; })"
  );
  check("取到 paired_grid 的显示名(供列表定位)", typeof pairName === "string" && pairName.length > 0, String(pairName));
  const copyClicked = await ev(
    "(() => { const rows = Array.from(document.querySelectorAll('#sg-listview .sg-row'));" +
      " const row = rows.filter(function (r) { const n = r.querySelector('.sg-row-name');" +
      " return n && n.textContent === " + JSON.stringify(pairName) + "; })[0];" +
      " if (!row) return 'no-row';" +
      " const btns = Array.from(row.querySelectorAll('.sg-row-actions button'));" +
      " const b = btns[btns.length - 1]; if (!b) return 'no-btn'; b.click(); return 'ok'; })()"
  );
  check("列表里内置行的「复制」可点", copyClicked === "ok", String(copyClicked));
  await sleep(800);
  const submitted = await ev(
    "(() => { const card = document.querySelector('.sg-modal-card'); if (!card) return 'no-card';" +
      " const ins = card.querySelectorAll('.sg-modal-form input.sg-input');" +
      " if (!ins.length) return 'no-input';" +
      " ins[0].value = " + JSON.stringify(copyName) + ";" +
      " const b = card.querySelector('.sg-modal-actions button.sg-btn-primary');" +
      " if (!b) return 'no-btn'; b.click(); return 'ok'; })()"
  );
  check("复制弹窗可填名并提交", submitted === "ok", String(submitted));
  await sleep(2000);
  const hashNow = await ev("location.hash");
  check("复制成功后落到副本详情页", hashNow === "#strategies/" + copyName, String(hashNow));
  const copyParamRows = await ev("document.querySelectorAll('#sg-params .sg-param').length");
  check("副本参数表单是满的(不是只剩一个交易对框)", Number(copyParamRows) >= 10, String(copyParamRows));
  const copyGuideHidden = await ev(
    "(() => { const g = document.getElementById('sg-params-guide'); return g ? g.hidden : null; })()"
  );
  check("副本页不显示参数引导卡(清单已随行)", copyGuideHidden === true, String(copyGuideHidden));
  // 参数行表头 = 「中文名 + 键名 + 必填标记」拼在一起(见 strategy_editor 的 renderParams) ——
  // 故这里判**包含**不判相等。中文名从副本自己的清单现取: 清单里的 name 是作者可改的,
  // 硬编码「间距百分比」会在改名人时变成假红。
  const wantLabel = await ev(
    "window.Ricow.api('/api/strategies/" +
      copyName +
      "').then(function (d) {" +
      " var p = (d.params || []).filter(function (x) { return x.key === 'spacing_pct'; })[0];" +
      " return p ? p.name : null; })"
  );
  check(
    "副本清单里 spacing_pct 带着中文显示名",
    typeof wantLabel === "string" && wantLabel.length > 0,
    String(wantLabel)
  );
  const copyLabels = await ev(
    "(() => Array.from(document.querySelectorAll('#sg-params .sg-param .sg-param-head'))" +
      ".map(function (n) { return n.textContent; }))()"
  );
  const labeled = Array.isArray(copyLabels) && copyLabels.some((t) => t.indexOf(wantLabel) >= 0);
  check(
    "副本参数带中文名(打开就能看懂, 不是裸键)",
    labeled,
    JSON.stringify(copyLabels).slice(0, 140)
  );

  // ---- ⑨ 053: 改参数必须亮「未保存」+ 给出改动前后对比 ----
  //
  // 这条复刻用户报的场景: "AI 修改策略以后, 修改后的参数反映在哪里, 需要点击保存吗?"
  // 修之前 `updateDirty` 只比代码编辑器, 参数改了(手动或 AI 改)界面一声不吭; 而回测又能拿
  // 表单值跑, 于是用户合理地以为已经改好了。以下逐条锁死修复后的行为。
  const beforeVal = await ev(
    "(() => { const rows = Array.from(document.querySelectorAll('#sg-params .sg-param'));" +
      " const row = rows.filter(function (r) { const k = r.querySelector('.sg-param-key');" +
      " return k && k.textContent === 'spacing_pct'; })[0];" +
      " if (!row) return 'no-row'; const ctl = row.querySelector('input, select');" +
      " if (!ctl) return 'no-input'; const v = ctl.value;" +
      " ctl.value = '0.05';" +
      " ctl.dispatchEvent(new Event('input', { bubbles: true }));" +
      " ctl.dispatchEvent(new Event('change', { bubbles: true }));" +
      " return v; })()"
  );
  check(
    "053 改参数: 找到 spacing_pct 输入框并改了值",
    typeof beforeVal === "string" && beforeVal !== "no-row" && beforeVal !== "no-input",
    String(beforeVal)
  );
  const pDirty = await ev("document.getElementById('sg-params-dirty').hidden");
  check("053 改参数后「参数未保存」提示亮起(修复前恒不亮)", pDirty === false, String(pDirty));
  const pDirtyText = await ev("document.getElementById('sg-params-dirty-text').textContent");
  check(
    "053 参数提示文案是「参数有未保存的修改」",
    pDirtyText === "参数有未保存的修改",
    String(pDirtyText)
  );
  // 只改了参数、没动代码 —— 代码卡那条不该亮(否则用户以为代码也被改了)。
  const codeDirty = await ev("document.getElementById('sg-dirty').hidden");
  check("053 只改参数时代码卡不误报未保存", codeDirty === true, String(codeDirty));
  const chgHidden = await ev("document.getElementById('sg-param-changes').hidden");
  check("053 改动对比面板出现", chgHidden === false, String(chgHidden));
  const chgText = await ev("document.getElementById('sg-param-changes').textContent");
  check(
    "053 改动面板同时给出改前值与改后值",
    String(chgText).indexOf(beforeVal) >= 0 && String(chgText).indexOf("0.05") >= 0,
    String(chgText).slice(0, 160)
  );
  check(
    "053 改动面板用中文名(不是裸键)",
    typeof wantLabel === "string" && String(chgText).indexOf(wantLabel) >= 0,
    String(chgText).slice(0, 120)
  );
  // 行内标记: 参数行自己也要标出「原 X → 改为 Y」, 参数多时面板会滚出视野。
  const inlineDiff = await ev(
    "(() => { const rows = Array.from(document.querySelectorAll('#sg-params .sg-param'));" +
      " const row = rows.filter(function (r) { const k = r.querySelector('.sg-param-key');" +
      " return k && k.textContent === 'spacing_pct'; })[0];" +
      " if (!row) return 'no-row'; const d = row.querySelector('.sg-param-diff');" +
      " return d ? (d.hidden ? 'hidden' : d.textContent) : 'no-diff'; })()"
  );
  check(
    "053 参数行行内标出「原 X → 改为 Y」",
    String(inlineDiff).indexOf(beforeVal) >= 0 && String(inlineDiff).indexOf("0.05") >= 0,
    String(inlineDiff)
  );
  // 一键还原: 改完能退回去, 且提示随之消失(否则"对比"给了却没法反悔)。
  const reverted = await ev(
    "(() => { const b = document.getElementById('sg-param-revert');" +
      " if (!b) return 'no-btn'; b.click();" +
      " const rows = Array.from(document.querySelectorAll('#sg-params .sg-param'));" +
      " const row = rows.filter(function (r) { const k = r.querySelector('.sg-param-key');" +
      " return k && k.textContent === 'spacing_pct'; })[0];" +
      " return row.querySelector('input, select').value; })()"
  );
  check("053 一键还原把值退回改前", reverted === beforeVal, String(reverted) + " 期望 " + beforeVal);
  const dirtyAfterRevert = await ev("document.getElementById('sg-params-dirty').hidden");
  check("053 还原后未保存提示消失", dirtyAfterRevert === true, String(dirtyAfterRevert));
  // 切英文后文案要跟着换(动态建的文字没有 data-zh 可依赖, 靠 refreshDynamic → updateDirty 重建)。
  await ev("window.Ricow.applyLang('en')");
  await sleep(400);
  await ev(
    "(() => { const rows = Array.from(document.querySelectorAll('#sg-params .sg-param'));" +
      " const row = rows.filter(function (r) { const k = r.querySelector('.sg-param-key');" +
      " return k && k.textContent === 'spacing_pct'; })[0];" +
      " const ctl = row.querySelector('input, select'); ctl.value = '0.07';" +
      " ctl.dispatchEvent(new Event('input', { bubbles: true })); return 'ok'; })()"
  );
  const enDirtyText = await ev("document.getElementById('sg-params-dirty-text').textContent");
  check(
    "053 切英文后未保存提示换成英文",
    enDirtyText === "Unsaved parameter changes",
    String(enDirtyText)
  );
  await ev("window.Ricow.applyLang('zh')");
  await sleep(300);

  checkNoErrors("052 模式 chip / 引导卡 / 识别参数 / 复制带清单");

  // ---- ⑩ 全局收口: 全程零未捕获异常 ----
  check("全程零未捕获异常/控制台错误", pageErrors.length === 0, pageErrors.slice(0, 5).join(" | "));
  if (faviconNoise > 0) {
    console.log(
      "  NOTE  已忽略 " +
        faviconNoise +
        " 条浏览器对 /favicon.ico 的自动请求(页面未声明 icon; 非产品路径, 051 复核的遗留项)"
    );
  }

  console.log("\n" + "=".repeat(78));
  console.log(fails === 0 ? "CDP 走查: 全绿" : "CDP 走查: " + fails + " 条失败");
  console.log("=".repeat(78));
  // 关掉本轮自己开的标签页(页面关掉后消息通道就断了, 故放在最后, 不 await 结果)。
  if (page.id) await fetch(CDP + "/json/close/" + page.id).catch(() => {});
  process.exit(fails === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error("走查脚本异常:", e);
  process.exit(2);
});
