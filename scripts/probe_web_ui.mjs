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
  let page = null;
  for (let i = 0; i < 40 && !page; i++) {
    try {
      const list = await (await fetch(CDP + "/json")).json();
      page = list.find((t) => t.type === "page");
    } catch (_) {}
    if (!page) await sleep(250);
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

  // ---- ⑧ 全局收口: 全程零未捕获异常 ----
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
  process.exit(fails === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error("走查脚本异常:", e);
  process.exit(2);
});
