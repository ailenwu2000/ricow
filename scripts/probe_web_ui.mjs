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
      // 带上 URL: 浏览器对「Failed to load resource」的 text 只有一句通用话(不含路径),
      // 只看 text 没法定位是哪个资源 404 —— 058 排查时就被这条挡过一次。
      pageErrors.push("[console.error] " + text + (m.params.entry.url ? " <" + m.params.entry.url + ">" : ""));
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
      " var r = rs.filter(function (x) { return x.id === 'shannon_grid_futures'; })[0];" +
      " return r ? r.name : null; })"
  );
  check("取到 shannon_grid_futures 的显示名(供列表定位)", typeof pairName === "string" && pairName.length > 0, String(pairName));
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
      " var p = (d.params || []).filter(function (x) { return x.key === 'min_spacing_pct'; })[0];" +
      " return p ? p.name : null; })"
  );
  check(
    "副本清单里 min_spacing_pct 带着中文显示名",
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
      " return k && k.textContent === 'min_spacing_pct'; })[0];" +
      " if (!row) return 'no-row'; const ctl = row.querySelector('input, select');" +
      " if (!ctl) return 'no-input'; const v = ctl.value;" +
      " ctl.value = '0.05';" +
      " ctl.dispatchEvent(new Event('input', { bubbles: true }));" +
      " ctl.dispatchEvent(new Event('change', { bubbles: true }));" +
      " return v; })()"
  );
  check(
    "053 改参数: 找到 min_spacing_pct 输入框并改了值",
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
      " return k && k.textContent === 'min_spacing_pct'; })[0];" +
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
      " return k && k.textContent === 'min_spacing_pct'; })[0];" +
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
      " return k && k.textContent === 'min_spacing_pct'; })[0];" +
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
  // 上面为了看英文文案又把值改成了 0.07, 页面**停在脏状态** —— 而 054 / 058 两段都假定能
  // 自由切视图。059 加了"带着未保存改动离开要拦一下"之后, 这个残留的脏会让它们全被拦住
  // (表象是 054 的详情页断言读到副本的摘要)。这里补一次还原: 让后续段回到"没改动"的前提,
  // 而"脏了会被拦"由 059 段**自己造脏**去验。
  await ev("(() => { const b = document.getElementById('sg-param-revert'); if (b) b.click(); return 'ok'; })()");
  await sleep(300);
  const cleanAgain = await ev("document.getElementById('sg-params-dirty').hidden");
  check("053 收尾: 探针把改动还原干净(后续段的前提)", cleanAgain === true, String(cleanAgain));

  // ---- ⑩ 054: AI 生成弹窗的结构化输入 / 验证窗口 / P4 验证卡 / T37 摘要落点 ----
  //
  // 这一轮新增的能力有一半是**行为**(档位下拉的选项来自后端、形态 B 的联动、摘要真的显示在
  // 详情页)。静态门只能证明那段字符串被服务出去了, 证明不了 DOM 真的长出来了 —— 所以走真机。
  await ev('location.hash = "#strategies"');
  await sleep(700);
  const genOpened = await ev(
    "(() => { const b = document.getElementById('sg-ai-new');" +
      " if (!b) return 'no-btn'; b.click(); return 'ok'; })()"
  );
  check("054 列表页「AI 生成」入口可点", genOpened === "ok", String(genOpened));
  await sleep(900);
  // 常驻风险提示条(P3-1): 不做勾选确认框, 但那几句话必须在场。
  const riskNotice = await ev(
    "(() => { const n = document.querySelector('.sg-modal-card .sg-risk-notice');" +
      " return n ? n.textContent : null; })()"
  );
  check(
    "054 生成弹窗有常驻风险提示条(不是空壳)",
    typeof riskNotice === "string" && riskNotice.length > 20,
    String(riskNotice).slice(0, 120)
  );
  // 档位下拉: 选项必须与后端 `/api/strategies/risk-profiles` **逐项一致**(前端不许手抄)。
  const tierOpts = await ev(
    "(() => { const s = document.getElementById('sg-gen-tier');" +
      " return s ? Array.from(s.options).map(function (o) { return o.value; }) : null; })()"
  );
  const tierApi = await ev(
    "window.Ricow.api('/api/strategies/risk-profiles').then(function (r) {" +
      " return (r.tiers || []).map(function (t) { return t.key; }); })"
  );
  check(
    "054 档位下拉的选项**就是**后端那几档(前端未手抄)",
    JSON.stringify(tierOpts) === JSON.stringify(tierApi),
    "dom=" + JSON.stringify(tierOpts) + " api=" + JSON.stringify(tierApi)
  );
  // 选中某档后, 含义文案要带上后端下发的**数字**(否则"稳一点"还是一句空话)。
  const tierMeaning = await ev(
    "(() => { const s = document.getElementById('sg-gen-tier');" +
      " s.value = 'conservative'; s.dispatchEvent(new Event('change', { bubbles: true }));" +
      " const h = document.querySelector('.sg-modal-card .sg-field-hint');" +
      " const rows = Array.from(document.querySelectorAll('.sg-modal-card .sg-field-hint'));" +
      " return rows.map(function (n) { return n.textContent; }).join(' | '); })()"
  );
  const consDoc = await ev(
    "window.Ricow.api('/api/strategies/risk-profiles').then(function (r) {" +
      " var t = (r.tiers || []).filter(function (x) { return x.key === 'conservative'; })[0];" +
      " return t ? t.meaning_zh : null; })"
  );
  check(
    "054 选档后显示后端给的「这档意味着什么」(含具体数字)",
    typeof consDoc === "string" && typeof tierMeaning === "string" &&
      tierMeaning.indexOf(consDoc) >= 0 && /[0-9]/.test(consDoc),
    "meaning=" + String(consDoc).slice(0, 120)
  );
  const customShown = await ev(
    "(() => { const s = document.getElementById('sg-gen-tier');" +
      " s.value = 'custom'; s.dispatchEvent(new Event('change', { bubbles: true }));" +
      " const r = document.getElementById('sg-gen-custom-row');" +
      " return r ? !r.hidden : null; })()"
  );
  check("054 选「自定义」档后手填字段出现", customShown === true, String(customShown));
  // P1: 验证窗口默认 90 天 —— 与回测卡/AI 评估同一默认, 数字才可比。
  const vDays = await ev(
    "(() => { const s = document.getElementById('sg-gen-verify-days'); return s ? s.value : null; })()"
  );
  check("054 验证窗口默认 90 天(与其余回测入口同默认)", vDays === "90", String(vDays));
  await ev(
    "(() => { const b = document.querySelector('.sg-modal-card .sg-modal-actions button');" +
      " if (b) b.click(); return 'ok'; })()"
  );
  await sleep(400);

  // P4 验证卡: 形态选择只在 walk 下出现, 形态 B 的提示只在 B 下出现(模式相关文案不能漏改)。
  await ev("location.hash = '#strategies/' + " + JSON.stringify(copyName));
  await sleep(1500);
  const valBoxExists = await ev("!!document.getElementById('sg-val-box')");
  check("054 P4 详情页有验证卡(sg-val-box)", valBoxExists === true, String(valBoxExists));
  const formHiddenAtSplit = await ev(
    "(() => { const m = document.getElementById('sg-val-mode');" +
      " m.value = 'split'; m.dispatchEvent(new Event('change', { bubbles: true }));" +
      " const l = document.getElementById('sg-val-form-label'); return l ? l.hidden : null; })()"
  );
  check("054 P4 mode=split 时不显示形态选择", formHiddenAtSplit === true, String(formHiddenAtSplit));
  const formShownAtWalk = await ev(
    "(() => { const m = document.getElementById('sg-val-mode');" +
      " m.value = 'walk'; m.dispatchEvent(new Event('change', { bubbles: true }));" +
      " const l = document.getElementById('sg-val-form-label'); return l ? l.hidden : null; })()"
  );
  check("054 P4 mode=walk 时形态选择出现", formShownAtWalk === false, String(formShownAtWalk));
  const bHint = await ev(
    "(() => { const f = document.getElementById('sg-val-form');" +
      " f.value = 'B'; f.dispatchEvent(new Event('change', { bubbles: true }));" +
      " const h = document.getElementById('sg-val-formb-hint');" +
      " return h ? (h.hidden ? 'hidden' : h.textContent) : null; })()"
  );
  check(
    "054 P4 选形态 B 时给出「每段前重新寻优」的说明(不让人以为参数是固定的)",
    typeof bHint === "string" && bHint !== "hidden" && bHint.length > 10,
    String(bHint).slice(0, 120)
  );
  // 回测卡的「不代表未来」免责(T25)只在**跑过回测**后随成本块渲染, 此处不重复断言
  // (真机走查不该为一条静态文案去跑一次几十秒的回测); 由 e2e L5 盯住它真的被引用。

  // T37 落点: 给一个 origin=ai 的策略写进摘要, 详情页要显示「这份策略在做什么」+ AI 徽章。
  const aiName = await ev("'zzaigen' + String(Date.now() % 100000000)");
  const aiSaved = await ev(
    "window.Ricow.api('/api/strategies', { method: 'POST', body: {" +
      " name: '" + aiName + "'," +
      " market: 'spot', pair: 'ETHUSDT'," +
      " code: 'function on_init(ctx) end\\nfunction on_tick(ctx) end\\n'," +
      " params: {}, overwrite: false, origin: 'ai', initial_cash: 10000 } })" +
      ".then(function () { return 'ok'; }).catch(function (e) { return 'ERR ' + e.message; })"
  );
  check("054 建一个 origin=ai 的探针策略", aiSaved === "ok", JSON.stringify(aiSaved).slice(0, 240));
  // 写入清单摘要 —— 这一步在真实链路里由服务端(AI 生成时)给出、前端随保存转发; 这里手工喂
  // 同一个端点, 验的是**详情页真的把它显示出来了**(T37 的落点: "这份策略在做什么")。
  const aiMf = await ev(
    "window.Ricow.api('/api/strategies/' + " + JSON.stringify(aiName) + " + '/manifest', { method: 'POST', body: {" +
      " name: '', summary: '按固定间距高抛低吸的网格', params: [] } })" +
      ".then(function () { return 'ok'; }).catch(function (e) { return 'ERR ' + e.message; })"
  );
  check("054 T37 摘要写进清单", aiMf === "ok", JSON.stringify(aiMf).slice(0, 240));
  await ev("location.hash = '#strategies/' + " + JSON.stringify(aiName));
  await sleep(1600);
  const fitText = await ev(
    "(() => { const b = document.getElementById('sg-fit-body'); return b ? b.textContent : null; })()"
  );
  check(
    "054 T37 详情页显示「这份策略在做什么」(摘要真的渲染出来)",
    typeof fitText === "string" && fitText.indexOf("按固定间距高抛低吸的网格") >= 0,
    String(fitText).slice(0, 160)
  );
  const aiBadge = await ev(
    "(() => { const b = document.querySelector('.sg-badge-ai');" +
      " return b ? b.textContent : null; })()"
  );
  check(
    "054 D8 详情页给 AI 生成的策略打「AI 生成」徽章",
    typeof aiBadge === "string" && aiBadge.length > 0,
    String(aiBadge)
  );
  // 收尾: 删掉探针策略(不把测试残留留给下一轮)。
  await ev(
    "window.Ricow.api('/api/strategies/' + " + JSON.stringify(aiName) + ", { method: 'DELETE' })" +
      ".then(function () { return 'ok'; }).catch(function () { return 'err'; })"
  );

  checkNoErrors("052 模式 chip / 引导卡 / 识别参数 / 复制带清单");

  // ---- ⑨' 058 kill switch: 运行视图入口 + 逐字短语联动 ----
  //
  // 为什么单列一段而不是并进 052: 这个按钮**带写动作**(POST /api/runs/kill), 与本页其余
  // 只读断言性质不同; 而且它的核心不变量是「短语没写对时按钮不放行」—— 这条只能真点一次
  // 才验得到, grep JS 文本永远验不出 `primary.disabled` 有没有真的被 syncHint 联动。
  // `R.router` **从未导出**(router.js 只导出 navigate/parseHash/refreshTitle), 所以此前
  // `window.Ricow.router && (location.hash = ...)` 这条恒短路 —— hash 根本没被赋值, 而本段
  // 的断言又只读 DOM 常驻元素(视图切换只是显隐), 于是"切到运行视图"是**假绿**: 一直停在
  // 上一段的策略详情页。改走真导出 `R.navigate`, 让视图真的切过去。
  await ev("window.Ricow.navigate('#runs'), 'ok'");
  await sleep(1200);
  const killBtn = await ev(
    "(() => { const b = document.getElementById('rn-kill-all');" +
      " return b ? b.textContent.trim() : null; })()"
  );
  check("058 运行视图有「停止全部」入口", killBtn === "停止全部", String(killBtn));

  // 点开弹窗: 必须能点开且给出短语输入框与不可逆提示。
  // 059 UX-09: 弹窗挂载点统一到首页的 `#modal-root`(原先 runs 视图自带的 `rn-modal-root`
  // 已从骨架移除)。这里跟着改成新挂载点 —— 对照组是"点开后弹窗内容真的在", 挂载点只是寻址。
  const opened = await ev(
    "(() => { const b = document.getElementById('rn-kill-all'); if (!b) return 'no-btn'; b.click();" +
      " const root = document.getElementById('modal-root');" +
      " return root && root.querySelector('input.sg-input') ? 'ok' : 'no-input'; })()"
  );
  check("058 点开「停止全部」弹窗且有逐字短语输入框", opened === "ok", String(opened));

  const warn = await ev(
    "(() => { const w = document.querySelector('.rn-kill-warn');" +
      " return w ? w.textContent : null; })()"
  );
  check(
    "058 弹窗写明后果(不是无声按下)",
    typeof warn === "string" && warn.length > 0,
    String(warn)
  );
  const keepDaemon = await ev(
    "(() => { const k = document.querySelector('.rn-kill-keep-daemon');" +
      " return k ? k.textContent : null; })()"
  );
  check(
    "058 弹窗说清 daemon 仍然活着(kill ≠ shutdown)",
    typeof keepDaemon === "string" && keepDaemon.indexOf("daemon") >= 0,
    String(keepDaemon)
  );

  // 核心不变量: 短语写错 → 按钮禁用; 写对 → 放行。后端是逐字比对(含平仓变体), 这里对齐前端。
  // 主按钮按**文案**定位而不是 `.sg-btn`: 弹窗里取消按钮也是 `sg-btn` 且默认未禁用,
  // 用类名挑会挑错对象, 断言就成了永远为 true 的假绿(058 第一版就栽在这)。
  const gate = await ev(
    "(() => { const root = document.getElementById('modal-root');" +
      " const input = root.querySelector('input.sg-input');" +
      " const btns = Array.from(root.querySelectorAll('button'));" +
      " const btn = btns.find(b => b.textContent.indexOf('停止全部') >= 0);" +
      " if (!input || !btn) return { err: 'missing', n: btns.length };" +
      " const set = (v) => { input.value = v; input.dispatchEvent(new Event('input', { bubbles: true })); };" +
      " set('y'); const afterY = btn.disabled;" +
      " set('确认停止全部 '); const afterSpace = btn.disabled;" +
      " set('确认停止全部'); const afterOk = btn.disabled;" +
      " const cb = root.querySelector('input[type=checkbox]');" +
      " cb.checked = true; cb.dispatchEvent(new Event('change', { bubbles: true }));" +
      " const cleared = input.value;" +
      " set('确认停止全部'); const afterPlainWhenCloseAll = btn.disabled;" +
      " set('确认停止全部并平仓'); const afterFull = btn.disabled;" +
      " return { afterY, afterSpace, afterOk, cleared, afterPlainWhenCloseAll, afterFull }; })()"
  );
  check("058 裸 y 不放行", gate && gate.afterY === true, JSON.stringify(gate));
  check("058 尾部多一个空格不放行(与后端逐字比对一致)", gate && gate.afterSpace === true, JSON.stringify(gate));
  check("058 短语逐字正确才放行", gate && gate.afterOk === false, JSON.stringify(gate));
  check("058 勾上平仓后输入框被清空(短语变了要重新打)", gate && gate.cleared === "", JSON.stringify(gate));
  check(
    "058 勾上平仓后旧短语不再放行",
    gate && gate.afterPlainWhenCloseAll === true,
    JSON.stringify(gate)
  );
  check("058 勾上平仓后要用更长的那个短语", gate && gate.afterFull === false, JSON.stringify(gate));

  // 关掉弹窗, 别把遮罩留给后续断言。按文案找取消钮(键 sgCancel → 「取消」),
  // 不要用 `!sg-btn-danger` 这类启发式: 主按钮本来就不带 danger 类, 挑不到。
  await ev(
    "(() => { const root = document.getElementById('modal-root');" +
      " const c = Array.from(root.querySelectorAll('button')).find(b => b.textContent.trim() === '取消');" +
      " if (c) c.click(); return 'ok'; })()"
  );
  await sleep(300);
  checkNoErrors("058 kill switch 入口与逐字短语联动");

  // ---- ⑪ 059 Web UI 体验改进 ----
  //
  // 这一批全是**纯前端**改动: HTTP 层冒烟(e2e_web.py)取回的只是一段字符串, 它证明不了
  // "焦点真的进了弹窗" / "卡片真的变高了"。所以每条都在真浏览器里点一次。
  const nav = async (v) => {
    await ev("window.Ricow.navigate('#" + v + "'), 'ok'");
    await sleep(900);
  };

  // UX-01 / UX-02: 顶栏标出当前视图 + 浏览器标签标题跟着走(此前 document.title 恒为 "Ricow",
  // 六个页签的标签页长得一模一样, 切了视图看不出自己在哪儿)。
  await nav("markets");
  const vt = await ev(
    "(() => ({ view: document.getElementById('app').dataset.view," +
      " label: (document.getElementById('view-title') || {}).textContent," +
      " docTitle: document.title }))()"
  );
  check(
    "059 顶栏标出当前视图名(此前六个页签长得一样)",
    typeof vt.label === "string" && vt.label.length > 0,
    JSON.stringify(vt)
  );
  check(
    "059 浏览器标签标题跟着视图走(此前恒为 Ricow)",
    typeof vt.docTitle === "string" && vt.docTitle !== "Ricow" && vt.docTitle.indexOf("Ricow") >= 0,
    String(vt.docTitle)
  );
  check("059 视图名写进 data-view(CSS 据此显隐会话区)", vt.view === "markets", String(vt.view));

  // UX-03: 会话列表/会话名只在对话视图露脸(此前六视图常驻, 在"市场"里挂一串对话标题)。
  const slHidden = await ev(
    "(() => { const l = document.getElementById('session-list');" +
      " return l ? getComputedStyle(l).display : 'missing'; })()"
  );
  check("059 非对话视图收起会话列表(此前六视图常驻)", slHidden === "none", String(slHidden));
  await nav("chat");
  const slShown = await ev(
    "(() => { const l = document.getElementById('session-list');" +
      " return l ? getComputedStyle(l).display : 'missing'; })()"
  );
  check("059 对话视图里会话列表仍然在", slShown !== "none", String(slShown));

  // UX-01 补验: 光"顶栏有几个字"证明不了它**跟着视图走** —— 一个恒定占位符同样能过上面那条。
  // 这里切到另一个视图再读一次: 两个名字必须不同, 且与侧栏那一项是同一份文案。
  const vt2 = await ev(
    "(() => ({ label: (document.getElementById('view-title') || {}).textContent," +
      " nav: (document.querySelector('#nav a[href=\"#chat\"]') || {}).textContent," +
      " docTitle: document.title }))()"
  );
  check(
    "059 切视图后顶栏名跟着变(不是恒定占位符)",
    typeof vt2.label === "string" && vt2.label.length > 0 && vt2.label !== vt.label,
    JSON.stringify({ markets: vt.label, chat: vt2.label })
  );
  check(
    "059 顶栏名与侧栏那一项同文案(防「导航写市场、顶栏写 Markets」的分叉)",
    typeof vt2.nav === "string" && vt2.nav.trim() === vt2.label.trim(),
    JSON.stringify(vt2)
  );
  check(
    "059 切视图后标签标题也跟着变",
    vt2.docTitle !== vt.docTitle && vt2.docTitle.indexOf("Ricow") >= 0,
    JSON.stringify({ markets: vt.docTitle, chat: vt2.docTitle })
  );

  // UX-09: 弹窗可访问性 —— 焦点进弹窗、Tab 出不去、Esc 关得掉、关掉后焦点回原处。
  // 此前两份 openModal 都没有 role/aria, 焦点留在背后页面, 且**非 Esc 关闭时监听不摘**
  // (每次开关都往 document 上叠一层 keydown)。
  await nav("strategies");
  // 真人点按钮时焦点本来就在按钮上; JS 的 `.click()` **不移动焦点**, 而 `restoreTo` 取的正是
  // 打开那一刻的 activeElement —— 不先 focus 就会把"关掉后焦点回原处"测成"回到 body",
  // 测的是脚本自己而不是产品。
  await ev(
    "(() => { const b = document.getElementById('sg-new'); if (!b) return 'no-btn';" +
      " b.focus(); b.click(); return 'ok'; })()"
  );
  await sleep(350);
  const dlg = await ev(
    "(() => { const d = document.querySelector('#modal-root [role=dialog]');" +
      " if (!d) return 'no-dialog';" +
      " const id = d.getAttribute('aria-labelledby');" +
      " return { modal: d.getAttribute('aria-modal')," +
      " labelled: !!(id && document.getElementById(id))," +
      " focusIn: d.contains(document.activeElement) }; })()"
  );
  check(
    "059 弹窗带 role=dialog 与 aria-modal(读屏器才知道这是个框)",
    dlg && dlg.modal === "true",
    JSON.stringify(dlg)
  );
  check("059 弹窗标题被 aria-labelledby 指向", dlg && dlg.labelled === true, JSON.stringify(dlg));
  check("059 打开后焦点进入弹窗(不是留在背后页面上)", dlg && dlg.focusIn === true, JSON.stringify(dlg));
  for (let i = 0; i < 4; i++) await key("Tab", "Tab", 9);
  const trapped = await ev(
    "(() => { const r = document.getElementById('modal-root');" +
      " return r ? r.contains(document.activeElement) : false; })()"
  );
  check("059 Tab 走不出弹窗(焦点陷阱)", trapped === true, String(trapped));
  await key("Escape", "Escape", 27);
  await sleep(350);
  const afterEsc = await ev(
    "(() => ({ open: !!document.querySelector('#modal-root [role=dialog]')," +
      " focus: document.activeElement ? document.activeElement.id : null }))()"
  );
  check("059 Esc 能关掉弹窗", afterEsc.open === false, JSON.stringify(afterEsc));
  check("059 关掉后焦点回到触发它的按钮(监听也一并摘掉)", afterEsc.focus === "sg-new", JSON.stringify(afterEsc));

  // UX-12: 卡片描述**真被截断**时才给「展开」, 点了就地展开(不跳详情), 再点收回。
  // 两条一起验: 截断与按钮显隐必须一致(不截断的行不该凭空多一个按钮), 且展开真的变高。
  const ux12 = await ev(
    "(() => { const rows = Array.from(document.querySelectorAll('.sg-row'));" +
      " let hit = null, mismatch = 0;" +
      " for (const row of rows) {" +
      "  const sum = row.querySelector('.sg-row-summary');" +
      "  const btn = row.querySelector('.sg-row-more');" +
      "  if (!sum || !btn) continue;" +
      "  const trunc = sum.scrollWidth > sum.clientWidth + 1;" +
      "  if (trunc !== !btn.hidden) mismatch += 1;" +
      "  if (trunc && !btn.hidden && !hit) hit = { row: row, sum: sum, btn: btn };" +
      " }" +
      " if (!hit) return { found: false, rows: rows.length, mismatch: mismatch };" +
      " const h0 = hit.row.getBoundingClientRect().height;" +
      " hit.btn.click();" +
      " const h1 = hit.row.getBoundingClientRect().height;" +
      " const mid = { aria: hit.btn.getAttribute('aria-expanded')," +
      "  cls: hit.sum.classList.contains('sg-row-summary-open'), txt: hit.btn.textContent.trim() };" +
      " hit.btn.click();" +
      " const h2 = hit.row.getBoundingClientRect().height;" +
      " return { found: true, rows: rows.length, mismatch: mismatch, h0: h0, h1: h1, h2: h2," +
      "  aria: mid.aria, cls: mid.cls, txt: mid.txt }; })()"
  );
  check("059 卡片描述被截断时才出现「展开」钮", ux12 && ux12.mismatch === 0, JSON.stringify(ux12));
  check("059 至少有一张卡片真的被截断(否则上面那条是空跑)", ux12 && ux12.found === true, JSON.stringify(ux12));
  check(
    "059 点「展开」后卡片变高(完整描述就地可见, 不跳详情)",
    ux12 && ux12.found && ux12.h1 > ux12.h0,
    JSON.stringify(ux12)
  );
  check(
    "059 展开态加了 open 类且 aria-expanded 为真",
    ux12 && ux12.cls === true && ux12.aria === "true",
    JSON.stringify(ux12)
  );
  // 「收起」这条必须带上"先真的展开过": 只比 h2≈h0 的话, 展开压根没生效时它照样绿
  // (M7 变异就是这么放过去的 —— 三条里红了两条, 唯独这条还在绿)。
  check(
    "059 再点一次收起(高度回到原样)",
    ux12 && ux12.found && ux12.h1 > ux12.h0 && Math.abs(ux12.h2 - ux12.h0) <= 1,
    JSON.stringify(ux12)
  );
  checkNoErrors("059 标题/弹窗/卡片展开");

  // UX-11: 命令面板「最近用过」—— 只记本次页面生命周期, 不落浏览器存储。
  await ev("(() => { const b = document.getElementById('palette-btn'); if (b) b.click(); return 'ok'; })()");
  await sleep(350);
  const firstGroups = await ev(
    "(() => Array.from(document.querySelectorAll('.palette-group')).map(e => e.textContent))()"
  );
  // 前置条件先钉住: 面板**真的**打开了。否则 `firstGroups` 是空数组,
  // 「没有『最近用过』」对空数组恒真 —— 面板压根没打开也会绿。
  check(
    "059 命令面板真的打开了(有分组可点, 否则下面两条是空跑)",
    Array.isArray(firstGroups) && firstGroups.length > 0,
    JSON.stringify(firstGroups)
  );
  check(
    "059 面板初次打开时没有「最近用过」(还没用过, 不摆空分组)",
    Array.isArray(firstGroups) && firstGroups.length > 0 && !firstGroups.some((g) => g.indexOf("最近用过") >= 0),
    JSON.stringify(firstGroups)
  );
  await key("Enter", "Enter", 13); // 执行高亮的第一条(视图跳转类, 无写动作)
  await sleep(500);
  await ev("(() => { const b = document.getElementById('palette-btn'); if (b) b.click(); return 'ok'; })()");
  await sleep(350);
  const secondGroups = await ev(
    "(() => Array.from(document.querySelectorAll('.palette-group')).map(e => e.textContent))()"
  );
  check(
    "059 执行过一条后再开面板, 顶部出现「最近用过」",
    Array.isArray(secondGroups) && secondGroups[0] === "最近用过",
    JSON.stringify(secondGroups)
  );
  await key("Escape", "Escape", 27);
  await sleep(250);
  checkNoErrors("059 命令面板最近用过");

  // UX-16: 有未保存改动时切走要拦一下(此前改了参数点侧栏就直接丢了)。
  await ev("location.hash = '#strategies/' + " + JSON.stringify(copyName));
  await sleep(900);
  const dirty = await ev(
    "(() => { const ins = Array.from(document.querySelectorAll('#sg-params input.sg-input'));" +
      " if (!ins.length) return 'no-input';" +
      " ins[0].value = '0.777'; ins[0].dispatchEvent(new Event('input', { bubbles: true }));" +
      " return 'ok'; })()"
  );
  // 前置条件先钉住: 改动必须**真的**被认成未保存 —— 否则下面几条不管过不过都说不清
  // (059 第一版就是这么假绿的: 探针改的是第一格输入框, 而"改没改"的判据没被验过)。
  const dirtyLit = await ev("document.getElementById('sg-params-dirty').hidden");
  check("059 改动被认成未保存(拦截断言的前置条件)", dirtyLit === false, String(dirtyLit));
  await ev("window.Ricow.navigate('#markets'), 'ok'");
  await sleep(800);
  const guard = await ev(
    "(() => { const d = document.querySelector('#modal-root [role=dialog]');" +
      " return { open: !!d, hash: location.hash, view: document.getElementById('app').dataset.view," +
      " text: d ? d.textContent.slice(0, 60) : null }; })()"
  );
  check("059 有未保存改动时切走会先问一句", guard.open === true, JSON.stringify(guard));
  check(
    "059 没放行前画面还在策略详情(不是先切走再问)",
    guard.view === "strategies" && guard.hash === "#strategies/" + copyName,
    JSON.stringify(guard)
  );
  // 主位是「留下去保存」: 点它应当原地不动。
  // 按钮按文案定位(不用 `.sg-btn-primary` 这类启发式): 文案就是给用户的那个承诺,
  // 且这里两个钮的差别正是"留下 vs 放弃", 按类名挑会挑错对象。注意是「留在这里」不是「留下」。
  const stayClick = await ev(
    "(() => { const r = document.getElementById('modal-root');" +
      " const b = Array.from(r.querySelectorAll('button')).find(x => x.textContent.indexOf('留在这里') >= 0);" +
      " if (b) b.click(); return b ? 'ok' : 'no-btn'; })()"
  );
  check("059 确认框的主位确实是「留在这里」那个", stayClick === "ok", String(stayClick));
  await sleep(700);
  const stayed = await ev(
    "(() => ({ hash: location.hash, modal: !!document.querySelector('#modal-root [role=dialog]') }))()"
  );
  check("059 选「留下」就还在详情页(改动没被丢掉)", stayed.hash === "#strategies/" + copyName, JSON.stringify(stayed));
  check("059 选完弹窗就关了(不挡着后续操作)", stayed.modal === false, JSON.stringify(stayed));
  // 另一条分支: 明确选「放弃改动, 离开」才真的走。
  await ev("window.Ricow.navigate('#markets'), 'ok'");
  await sleep(800);
  await ev(
    "(() => { const r = document.getElementById('modal-root');" +
      " const b = Array.from(r.querySelectorAll('button')).find(x => x.textContent.indexOf('放弃改动') >= 0);" +
      " if (b) b.click(); return b ? 'ok' : 'no-btn'; })()"
  );
  await sleep(900);
  const left = await ev(
    "(() => ({ hash: location.hash, view: document.getElementById('app').dataset.view," +
      " modal: !!document.querySelector('#modal-root [role=dialog]') }))()"
  );
  check(
    "059 选「放弃改动, 离开」才真的走掉",
    left.hash === "#markets" && left.view === "markets" && left.modal === false,
    JSON.stringify(left)
  );
  checkNoErrors("059 未保存离开确认");

  // ---- ⑪ 补验: UX-05 / UX-06 / UX-07 / UX-13 ----
  //
  // 这四项 059 都动了, 但第一批断言**一条都没覆盖**。写第一批时只盯着"肉眼看得见"的那几处,
  // 于是"改了什么"和"验了什么"是两回事。这里补齐, 免得下轮有人改动它们而走查照样全绿。

  // UX-05: `R.api` 的前端超时。默认**不启用**(回测等长作业靠服务端 202 + 轮询, 不该被前端掐表),
  // 传了才掐。用 stub 掉的 `fetch` 造一个永不返回的上游 —— 真实网络里那叫"连不上 / 服务没了",
  // 界面上原本的表现是永远停在「加载中…」, 且没有任何可点的东西。
  // 外面再套一层 5s 兜底: 万一超时没生效, 是断言红而不是整轮脚本卡死。
  // stub 必须**尊重 signal**: 真实的 fetch 被 abort 时会 reject 一个 AbortError, 而这里正是靠
  // 这一点区分"产品真的传了 signal" 和 "产品压根没掐表" —— 后者在这个 stub 下会永远挂着
  // (于是落到外层 5s 兜底, 断言红)。
  const STUB =
    "window.fetch = (p, o) => new Promise((_, rej) => {" +
    " if (o && o.signal) o.signal.addEventListener('abort', () => {" +
    "  const e = new Error('The user aborted a request.'); e.name = 'AbortError'; rej(e); }); });";
  const to = await ev(
    "(async () => { const orig = window.fetch;" +
      STUB +
      " let msg = 'NO-THROW', ms = -1; const t0 = performance.now();" +
      " try { await Promise.race([window.Ricow.api('/api/probe-never', { timeoutMs: 80 })," +
      "  new Promise((_, rej) => setTimeout(() => rej(new Error('PROBE-GUARD')), 5000))]); }" +
      " catch (e) { msg = e && e.message ? e.message : String(e); ms = performance.now() - t0; }" +
      " window.fetch = orig; return { msg: msg, ms: ms }; })()"
  );
  check(
    "059 请求超时后给中文原因(不是浏览器那句 The user aborted a request)",
    to && typeof to.msg === "string" && to.msg.indexOf("超时") >= 0,
    JSON.stringify(to)
  );
  check(
    "059 超时按传入的上限发生(既不是立刻假超时, 也不是永远挂着)",
    to && to.ms >= 70 && to.ms < 4000,
    JSON.stringify(to)
  );
  const noTo = await ev(
    "(async () => { const orig = window.fetch;" +
      STUB +
      " let settled = false;" +
      " window.Ricow.api('/api/probe-never').then(() => { settled = true; }).catch(() => { settled = true; });" +
      " await new Promise(r => setTimeout(r, 400));" +
      " window.fetch = orig; return settled; })()"
  );
  check("059 不传上限时沿用旧行为(长作业不被前端掐表)", noTo === false, String(noTo));
  checkNoErrors("059 前端超时(UX-05)");

  // UX-06: 空状态统一。此前每个视图各写各的(有的只有一句灰字, 有的干脆空白)。
  const es = await ev(
    "(() => { const a = window.Ricow.emptyState('x');" +
      " const b = window.Ricow.emptyState('y', { label: '去策略页', view: '#strategies' });" +
      " return { plain: a.className, hasBtn: !!b.querySelector('.empty-cta-btn')," +
      " btnText: (b.querySelector('.empty-cta-btn') || {}).textContent }; })()"
  );
  check("059 空状态统一走 R.emptyState(无 cta 时就是一句灰字)", es && es.plain === "empty", JSON.stringify(es));
  check(
    "059 空状态给 cta 时多出一个可点按钮(不是干瞪眼)",
    es && es.hasBtn === true && es.btnText === "去策略页",
    JSON.stringify(es)
  );
  checkNoErrors("059 空状态(UX-06)");

  // UX-13: 「停止全部」是不可逆的危险动作, 没有任何实例时它无事可停 —— 不该亮在空表格上方。
  // 判据写成"显隐 ⟺ 有没有实例", 这样不管本轮有没有实例都验到了逻辑, 而不是碰运气。
  await nav("runs");
  const ux13 = await ev(
    "(() => { const b = document.getElementById('rn-kill-all');" +
      " if (!b) return 'no-btn';" +
      " const rows = document.querySelectorAll('#rn-tbody tr');" +
      " let real = 0;" +
      " for (const r of rows) { if (!r.classList.contains('rn-empty')) real += 1; }" +
      " return { hidden: b.hidden, real: real }; })()"
  );
  check(
    "059 「停止全部」的显隐跟着实例数走(空页面不摆最危险的动作)",
    ux13 && ux13.hidden === (ux13.real === 0),
    JSON.stringify(ux13)
  );
  checkNoErrors("059 危险动作按状态显隐(UX-13)");

  // UX-07: 密钥页「0 条已保存」与「当前生效: …」并排出现过, 看着像数据自相矛盾 ——
  // 其实是两样东西(密钥环 vs 正在生效的那一段), 但页面从没说过。补的那句要真的渲染出来。
  await nav("keys");
  const khZh = await ev(
    "(() => { const p = document.querySelector('.keys-ring-hint'); return p ? p.textContent : 'missing'; })()"
  );
  check(
    "059 密钥页说清「密钥环」与「当前生效」是两样东西",
    typeof khZh === "string" && khZh.indexOf("密钥环") >= 0 && khZh.indexOf("当前生效") >= 0,
    String(khZh).slice(0, 70)
  );
  await ev("window.Ricow.applyLang('en')");
  await sleep(250);
  const khEn = await ev(
    "(() => { const p = document.querySelector('.keys-ring-hint'); return p ? p.textContent : 'missing'; })()"
  );
  await ev("window.Ricow.applyLang('zh')");
  await sleep(250);
  check(
    "059 切英文后这句也是英文(没留一段中文在英文界面上)",
    typeof khEn === "string" && khEn.indexOf("key ring") >= 0 && khEn.indexOf("currently active") >= 0,
    String(khEn).slice(0, 70)
  );
  checkNoErrors("059 密钥页说明(UX-07)");

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
