// Ricow Web 前端公共件 (032): 单命名空间 `window.Ricow` —— TOKEN / api / TEXT 字典 /
// 通用 i18n 与纯工具。032 从原 app.js 整体抽出, 逻辑零改写。
//
// 与 CLI 同源: 前端**不做任何业务判断**, 所有会话内容都经 `/api/**` + token 拿,
// 页面自身不落任何状态到磁盘。
"use strict";

(function () {
  const R = (window.Ricow = window.Ricow || {});

  // 页面 URL 上的一次性 token(与 `<script src="?token=">` 同一条取值路径, 见 web/mod.rs)。
  //
  // 取两处, 顺序不能反(043): 顶部导航带 `?token=` 进来时, 中间件 303 到去掉 token 的路径
  // (安全-11: 地址栏/历史不留 token), 此后 `location.search` 里**就没有** token 了。
  // `R.api` 还能靠 cookie 活着, 但 `EventSource`/WebSocket 无法自定请求头, 只能把 token 挂 URL 上 ——
  // 而 "?token=" + "" 拼出来的空 query token 会**顶掉**后面那条有效的 cookie
  // (`auth.rs::token_of` 里 query 优先于 cookie, 且不判空) → 全部流式连接 401。
  // 兜底载体由服务端在渲染首页时填入(见 index.html 的 meta 注释), 与资源 URL 用的是同一个 token。
  R.TOKEN =
    new URLSearchParams(location.search).get("token") ||
    (document.querySelector('meta[name="ricow-token"]') || {}).content ||
    "";

  // 运行时文案(页面静态标签走 `data-zh` / `data-en`, 见 `R.applyLang`)。
  R.TEXT = {
    zh: {
      thinking: "正在思考…",
      ended: "会话已结束, 请点左上角新建会话。",
      failed: "请求失败: ",
      untitled: "新会话",
      newSession: "新会话",
      delete: "删除会话",
      confirmDelete: "删除该会话? 它的消息会一并删除, 且无法恢复。",
      termHint: "点击查看解释",
      termClose: "点术语看解释 · 点别处关闭",
      asof: "截至 ",
      daemonDown: "daemon 未运行 —— 下面是它停机前的最后一次记录, 此刻状态不可知。",
      unreadable: "读不到",
      unknown: "状态不可知",
      emptyPositions: "无持仓",
      emptyOrders: "无挂单",
      emptyFills: "无成交",
      lEntry: "开仓价",
      lMode: "模式",
      lFilled: "已成交",
      lStatus: "状态",
      lFee: "手续费",
      lOrderId: "订单号",
      lUnknown: "未知",
      noLogs: "还没有任何策略日志(策略启动后才会生成)。",
      rotated: "日志已轮转(或被截断), 已从头重读 —— 不丢行、不重复",
      logUnreadable: "日志读不到",
      // 044 日志工具条: 级别过滤是**行内关键字启发式**, 文案必须自陈 —— 日志行是文件里的原样
      // 文本(FR-018 不解析), 说成"精确分级"就是骗人。
      logLevelHint: "级别按行内关键字粗略匹配 —— 日志是原样文本, 不做解析",
      logFollow: "跟随中",
      logPaused: "已暂停",
      logNew: "条新",
      logCount: "显示 {shown} / 共 {total} 行 · 本地最多保留 {cap} 行",
      spot: "现货",
      futures: "合约",
      noStrategies: "无策略",
      required: "必填",
      suitable: "适合: ",
      unsuitable: "不适合: ",
      emptySessions: "还没有会话 —— 用大白话问我任何事就行, 不必记命令。",
      goStrategies: "去策略页",
    },
    en: {
      thinking: "thinking…",
      ended: "Session ended — start a new one from the top-left.",
      failed: "Request failed: ",
      untitled: "New session",
      newSession: "New session",
      delete: "Delete session",
      confirmDelete: "Delete this session? Its messages go too, and this cannot be undone.",
      termHint: "Click for the explanation",
      termClose: "Click a term for its meaning · click elsewhere to close",
      asof: "as of ",
      daemonDown: "daemon is not running — what follows is its last record before shutdown; the current state is unknown.",
      unreadable: "unreadable",
      unknown: "state unknown",
      emptyPositions: "No positions",
      emptyOrders: "No open orders",
      emptyFills: "No fills",
      lEntry: "entry",
      lMode: "mode",
      lFilled: "filled",
      lStatus: "status",
      lFee: "fee",
      lOrderId: "order id",
      lUnknown: "unknown",
      noLogs: "No strategy logs yet (a log appears once a strategy has been started).",
      rotated: "Log rotated (or truncated); re-read from the start — no line dropped or repeated",
      logUnreadable: "Log unreadable",
      logLevelHint: "Levels matched by keywords in the line text — log lines are shown verbatim, never parsed",
      logFollow: "Following",
      logPaused: "Paused",
      logNew: "new",
      logCount: "showing {shown} / {total} lines · at most {cap} kept locally",
      spot: "Spot",
      futures: "Futures",
      noStrategies: "No strategies",
      required: "required",
      suitable: "Fits: ",
      unsuitable: "Unfits: ",
      emptySessions: "No sessions yet — just ask me anything in plain language; no commands to memorise.",
      goStrategies: "Go to Strategies",
    },
  };

  // 当前界面语言(单一来源): 由 `R.applyLang` 写, 各模块读 —— 不另立第二份语言状态。
  R.lang = "zh";

  R.t = (key) => R.TEXT[R.lang][key];

  // ---------- 基础 ----------

  /// 统一取数: 自动带 Bearer token; body 对象自动 JSON 化。401 / 非 2xx 一律抛错。
  /// 错误体优先解析服务端 `WebError` JSON(`{error, code?, line?}`,032 US3 起统一格式):
  /// err.message 为中文一句(供行内展示),err.status/code/line 供调用方分支处理
  /// (如 code:"running"/"compile");非 JSON 老响应回退原文。
  /// `204 No Content`(如 POST /api/config/keys)无 JSON 体, 返回 null。
  R.api = async function (path, options) {
    const opts = Object.assign({}, options);
    opts.headers = Object.assign({ Authorization: "Bearer " + R.TOKEN }, opts.headers || {});
    if (opts.body !== undefined) {
      opts.headers["Content-Type"] = "application/json";
      opts.body = JSON.stringify(opts.body);
    }
    const res = await fetch(path, opts);
    if (!res.ok) {
      let detail = "";
      try {
        detail = await res.text();
      } catch (_) {
        detail = "";
      }
      // 401 中间件短路:空体。其余 WebError 为 JSON;解析失败再回退纯文本。
      let parsed = null;
      if (detail) {
        try {
          parsed = JSON.parse(detail);
        } catch (_) {
          parsed = null;
        }
      }
      const message =
        parsed && typeof parsed.error === "string" && parsed.error
          ? parsed.error
          : detail || res.status + " " + res.statusText;
      const err = new Error(message);
      err.status = res.status;
      if (parsed) {
        if (typeof parsed.code === "string") err.code = parsed.code;
        if (typeof parsed.line === "number") err.line = parsed.line;
        if (typeof parsed.disclosure === "string") err.disclosure = parsed.disclosure;
      }
      throw err;
    }
    if (res.status === 204) return null;
    return res.json();
  };

  /// 统一 SSE 打开器(036): 指数退避 + 抖动的重连。
  ///
  /// 浏览器 `EventSource` 自带重连但**不退避也不封顶**(服务真停了会按固定节奏永远打);
  /// 且原生重连只在**同一个** EventSource 对象上自动携带 `Last-Event-ID`, 这里接管后
  /// 重开的是新对象 —— 故记录最后一帧的 id, 重开时拼进 `last_event_id` 查询参数
  /// (日志流服务端读它续传, 会话流忽略之)。`url` 须已带查询串(`?token=...`), 追加用 `&`。
  /// 连续失败 5 次(1s → 2s → 4s → 8s 封顶, ±20% 抖动)放弃并回调 `onfail` 一次;
  /// 任一次连上(`onopen`)即清零重计。返回 `{ close() }`。
  R.sse = function (url, handlers) {
    const MAX_RETRIES = 5;
    let es = null;
    let attempts = 0;
    let lastId = null;
    let timer = null;
    let closed = false;

    function open() {
      if (closed) return;
      const target =
        lastId === null ? url : url + "&last_event_id=" + encodeURIComponent(lastId);
      es = new EventSource(target);
      es.onopen = () => {
        attempts = 0;
        if (handlers.onopen) handlers.onopen();
      };
      es.onmessage = (ev) => {
        if (ev.lastEventId) lastId = ev.lastEventId;
        if (handlers.onmessage) handlers.onmessage(ev);
      };
      es.onerror = () => {
        es.close();
        es = null;
        if (attempts >= MAX_RETRIES) {
          if (handlers.onfail) handlers.onfail();
          return;
        }
        const base = Math.min(1000 * Math.pow(2, attempts), 8000);
        const wait = base * (0.8 + Math.random() * 0.4); // ±20% 抖动
        attempts += 1;
        timer = setTimeout(open, wait);
      };
    }
    open();
    return {
      close() {
        closed = true;
        if (timer) clearTimeout(timer);
        if (es) es.close();
        es = null;
      },
    };
  };

  /// 双语切换的**通用部分**(FR-027 / FR-028): 静态标签就地换, 语言值本身存在
  /// `ricow.toml` 的 `[ui].lang`。各视图自己的重渲染(会话列表 / 面板等)由各模块在
  /// 调完本函数后追加, 不在这里耦合。
  ///
  /// `dataset` 的键名是**首字母小写**的驼峰: `data-zh` → `dataset.zh`、`data-zh-placeholder` →
  /// `dataset.zhPlaceholder`。用大写开头取到的是 `undefined` —— `textContent` 是可空类型会静默
  /// 变空字符串(按钮/提示语整片空白), `placeholder` 是不可空字符串会变成字面量 "undefined"。
  R.applyLang = function (lang) {
    R.lang = lang === "en" ? "en" : "zh";
    document.documentElement.lang = R.lang === "en" ? "en" : "zh-CN";
    const langBtn = document.getElementById("lang-toggle");
    if (langBtn) langBtn.textContent = R.lang === "zh" ? "EN" : "中";
    const suffix = R.lang;
    for (const node of document.querySelectorAll("[data-zh]")) {
      node.textContent = node.dataset[suffix];
    }
    for (const node of document.querySelectorAll("[data-zh-placeholder]")) {
      node.placeholder = node.dataset[suffix + "Placeholder"];
    }
    // 解释浮层开着就地换语言, 不必关掉重开(它是全局单例, 见下面的 R.glossary)。
    if (R.glossary) R.glossary.refresh();
  };

  // ---------- 纯工具 ----------

  // ---------- 主题(034 / D14) ----------
  // 两个概念要分清, 混起来就会出现"选了跟随系统, 关掉页面再打开又变回深色":
  //   · `R.theme` = **偏好**(落盘在 `ricow.toml` 的 `[ui].theme`): dark / light / red / **auto**;
  //   · `<html data-theme>` = **实际生效的配色**: dark / light / red —— `auto` 在这里按系统的
  //     `prefers-color-scheme` 解成 dark 或 light。
  //
  // 为什么不给 `auto` 单独写一份 CSS 变量: 那等于把浅色那套维护两遍, 系统一换色两边迟早分叉。
  // 解成既有值之后, CSS 侧**一行都不用动**(`:root` 深色 + light / red 两个覆盖块照旧)。
  const prefersLight =
    window.matchMedia && window.matchMedia("(prefers-color-scheme: light)");

  /// 偏好 → 实际生效的配色。
  function resolveTheme(pref) {
    if (pref === "light" || pref === "red") return pref;
    if (pref === "auto") return prefersLight && prefersLight.matches ? "light" : "dark";
    return "dark";
  }

  R.theme = "dark"; // 偏好(单一来源: ricow.toml 的 [ui].theme)

  R.applyTheme = function (theme) {
    R.theme = theme === "light" || theme === "red" || theme === "auto" ? theme : "dark";
    const applied = resolveTheme(R.theme);
    document.documentElement.dataset.theme = applied;
    const sel = document.getElementById("theme-select");
    if (sel) sel.value = R.theme; // 控件显示**偏好**(选了"跟随系统"就一直显示它)
    // 广播给关心外观的模块(如 markets.js 的 K 线图要按新变量重取配色); 传的是**实际配色**,
    // 因为消费者读的是 CSS 变量。
    window.dispatchEvent(new CustomEvent("ricow:theme", { detail: applied }));
  };

  // 系统深/浅色变了: 只有"跟随系统"这一档需要重画(另外三档与此无关)。
  if (prefersLight) {
    const onScheme = () => {
      if (R.theme === "auto") R.applyTheme("auto");
    };
    if (prefersLight.addEventListener) prefersLight.addEventListener("change", onScheme);
    else if (prefersLight.addListener) prefersLight.addListener(onScheme); // 老引擎
  }

  // ---------- 格式化: 全站唯一出口 (C11) ----------
  //
  // 原本 `formatNum`(市场盘口) / `fmtPct`·`fmtNum`·`fmtMs`(回测) / `formatStamp`(对话) /
  // `R.formatTime`(会话列表) 各写一份, 口径分歧只靠"记得别用错那个"来维持 —— 迟早出错。
  // 现在统一到这里: **差异用不同方法表达**(num / signed / pct / time / timeSec), 而不是
  // 让每个调用点各写一遍。各视图的局部函数(若有)一律改成委托, 调用点不变。
  //
  // 两条口径**故意不合并**(合并就会把一种场景显示错):
  //   · `timeSec`(秒级, 会话列表) 缺值 → `""`(列表项不显示时间); `time`(毫秒, 交易明细)
  //     缺值 → `"—"`(表格里空着会被当成数据缺失)。
  //   · 金融数字一律用**点**作小数点, 不引千分位 —— 表格按位对齐比好看更要紧。
  R.fmt = (function () {
    const locale = () => (R.lang === "en" ? "en-US" : "zh-CN");
    const stampOpts = { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" };
    const stampOptsSec = {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    };
    return {
      /// 无符号数字: 按 `digits` 位截断再去尾零(`0.30000000000000004` → `0.3`)。
      /// 缺值 / 非有限 → `—`(绝不把 NaN / Infinity 显示出去)。
      num: function (v, digits) {
        const n = Number(v);
        if (v == null || !Number.isFinite(n)) return "—";
        const d = typeof digits === "number" ? digits : 8;
        return String(Number(n.toFixed(d)));
      },
      /// 带符号数字(盈亏类): 正数补 `+`; 位数随量级(≥100 两位 / ≥1 四位 / 其余六位)。
      signed: function (v) {
        if (v == null || !Number.isFinite(v)) return "—";
        const abs = Math.abs(v);
        const digits = abs >= 100 ? 2 : abs >= 1 ? 4 : 6;
        return (v > 0 ? "+" : "") + v.toFixed(digits).replace(/\.?0+$/, "");
      },
      /// 百分比(收益率类): 带符号 + 两位小数 + `%`。
      pct: function (v) {
        if (v == null || !Number.isFinite(v)) return "—";
        return (v > 0 ? "+" : "") + v.toFixed(2) + "%";
      },
      /// 秒数 → 紧凑时长(`3d 4h` / `2h 5m` / `7m 12s` / `42s`)。缺值 → `--`。
      dur: function (secs) {
        if (secs === null || secs === undefined) return "--";
        const s = Number(secs);
        if (!Number.isFinite(s)) return "--";
        const d = Math.floor(s / 86400);
        const hh = Math.floor((s % 86400) / 3600);
        const mm = Math.floor((s % 3600) / 60);
        const ss = s % 60;
        if (d > 0) return d + "d " + hh + "h";
        if (hh > 0) return hh + "h " + mm + "m";
        if (mm > 0) return mm + "m " + ss + "s";
        return ss + "s";
      },
      /// 毫秒时间戳 → 本地化展示串(交易明细 / 平仓明细)。非有限 → `—`。
      /// `withSeconds` 为真时多给秒 —— 对话里的成交 / 挂单时间戳要按秒区分先后。
      time: function (ms, withSeconds) {
        if (!Number.isFinite(ms)) return "—";
        return new Date(ms).toLocaleString(locale(), withSeconds ? stampOptsSec : stampOpts);
      },
      /// 秒级时间戳 → 本地化展示串(会话列表的 `updated_at`)。缺值 → `""`。
      timeSec: function (ts) {
        if (!ts) return "";
        return new Date(ts * 1000).toLocaleString(locale(), stampOpts);
      },
    };
  })();

  /// 兼容既有调用: `R.formatTime` 就是秒级口径(C11 起由 `R.fmt.timeSec` 提供)。
  R.formatTime = function (ts) {
    return R.fmt.timeSec(ts);
  };

  // ---------- 空状态 ----------
  //
  // B4: 空状态最容易做成的样子是一行灰字("无策略" / "—" / "无"), 用户看完还是不知道该干嘛。
  // 统一收在这里: 一句说明 + 一个「下一步」按钮。
  //
  // 按钮只做**导航 / 打开已有入口**, 绝不替用户写入(与"写动作必须本人确认"同一姿态);
  // 没有 `cta` 时**原样返回一个 `.empty` 灰字块** —— 调用点不必为"有没有下一步"分两套代码。
  R.emptyState = function (text, cta) {
    const note = document.createElement("div");
    note.className = "empty";
    note.textContent = text;
    if (!cta || !cta.label) return note;
    const box = document.createElement("div");
    box.className = "empty-cta";
    box.appendChild(note);
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "empty-cta-btn";
    btn.textContent = cta.label;
    btn.addEventListener("click", () => {
      if (cta.onClick) cta.onClick();
      else if (cta.view) R.navigate(cta.view);
    });
    box.appendChild(btn);
    return box;
  };

  // ---------- 全局通知(toast) ----------
  //
  // B6: 各视图原先各自把提示写进自己的 msg 区 —— 一旦**跨视图**(复制完自动打开详情、
  // 保存完回列表), 提示就随旧 DOM 一起没了, 用户会以为"是不是没保存上"。这里给一个挂在
  // `document.body` 上的**单一出口**: 切视图 / 重渲染都不影响它。
  //
  // 只用于**成功 / 中性**一路; 错误留在原地(msg 区)显示 —— 错误是"这一屏此刻的状态",
  // 需要贴着出错的控件看, 飘到右下角反而不对。
  // `aria-live="polite"` 保住可访问性; 点一下即收, 到点自动收。
  let toastHost = null;
  const TOAST_MS = 4000;

  R.toast = function (msg, kind) {
    if (!msg) return;
    if (!toastHost) {
      toastHost = document.createElement("div");
      toastHost.className = "toast-host";
      toastHost.setAttribute("role", "status");
      toastHost.setAttribute("aria-live", "polite");
      document.body.appendChild(toastHost);
    }
    const item = document.createElement("div");
    item.className = "toast toast-" + (kind || "info");
    item.textContent = msg; // 服务端文本只走 textContent(与全站同一条红线)
    item.addEventListener("click", () => item.remove());
    toastHost.appendChild(item);
    setTimeout(() => item.remove(), TOAST_MS);
  };

  // ---------- 术语表: 全局单例 ----------
  //
  // 术语解释**只有一份来源**: `GET /api/terms`(后端 `web/terms.rs`)。此前只有对话视图取它,
  // 也只把解释挂在自己的气泡上 —— 同一个「最大回撤」在回测指标卡上就点不了。取数 / 索引 /
  // 解释浮层都收在这里, 各视图一律 `R.glossary.decorate(root)` 挂载, 前端**不另立第二份定义**:
  // 匹配用的就是术语表自己的 key(与后端报告指标名逐字同源), 所以只有**真在术语表里的名字**
  // 会变成可点词条, 其余文本一字不动。
  //
  // 安全: 服务端文本只走 `textContent`; 无内联事件; 不碰浏览器存储。
  R.glossary = (function () {
    const index = new Map(); // key → {key, zh, en}
    let re = null; // 全部 key 拼成的匹配式(长名优先)
    let pending = null; // 取数中的 promise(并发调用共用一次请求)
    let pop = null; // 解释浮层(单例, 首次用到才建)
    let current = null; // 浮层当前显示的 key

    const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

    /// 取一次并建索引(`/api/terms` 已把中英两份一起给全, 切语言不重取)。失败不粘住, 可重试。
    function load() {
      if (pending) return pending;
      pending = R.api("/api/terms").then((rows) => {
        const keys = [];
        for (const term of rows) {
          index.set(term.key, term);
          keys.push(term.key);
        }
        // 长名优先: `手续费占比` 不该被 `手续费` 从中间截断。
        keys.sort((a, b) => b.length - a.length);
        re = keys.length ? new RegExp(keys.map(escapeRe).join("|"), "g") : null;
        return rows;
      });
      pending.catch(() => {
        pending = null;
      });
      return pending;
    }

    const has = (key) => index.has(key);
    const all = () => Array.from(index.values()); // 保持服务端顺序
    const text = (key) => {
      const term = index.get(key);
      return term ? (R.lang === "en" ? term.en : term.zh) : "";
    };

    /// 术语按钮: 词条名 + 点击开解释。`class="term"` 既是既有样式, 也是"已装饰"的标记。
    function button(key) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "term";
      btn.textContent = key;
      btn.title = R.t("termHint");
      btn.addEventListener("click", (ev) => {
        ev.stopPropagation(); // 点术语是"看解释", 不该顺带触发展开 / 跳转
        open(key, btn);
      });
      return btn;
    }

    function ensurePop() {
      if (pop) return pop;
      pop = document.createElement("div");
      pop.className = "term-pop";
      pop.hidden = true;
      document.body.appendChild(pop);
      // 关闭手势: 点浮层外 / Esc / 改窗宽 / 任何滚动(滚动后位置不再贴合)。
      // 捕获阶段听滚动 —— 元素上的 scroll 不冒泡, 但捕获阶段仍会经过 window, 故一个监听覆盖全站。
      document.addEventListener("click", (ev) => {
        if (pop.contains(ev.target)) return; // 浮层内的点击(选文字)不该关掉它
        close();
      });
      document.addEventListener("keydown", (ev) => {
        if (ev.key === "Escape") close();
      });
      window.addEventListener("resize", close);
      window.addEventListener("scroll", close, { capture: true, passive: true });
      return pop;
    }

    function render() {
      const term = index.get(current);
      if (!term) return;
      pop.textContent = "";
      const name = document.createElement("div");
      name.className = "term-name";
      name.textContent = term.key;
      const body = document.createElement("div");
      body.className = "term-body";
      body.textContent = R.lang === "en" ? term.en : term.zh;
      const hint = document.createElement("div");
      hint.className = "term-hint";
      hint.textContent = R.t("termClose");
      pop.appendChild(name);
      pop.appendChild(body);
      pop.appendChild(hint);
    }

    /// 贴在词条下方; 下方放不下就翻到上方, 并夹在视口内。
    function place(anchor) {
      const rect = anchor.getBoundingClientRect();
      let left = Math.min(rect.left, window.innerWidth - pop.offsetWidth - 8);
      left = Math.max(8, left);
      let top = rect.bottom + 6;
      if (top + pop.offsetHeight > window.innerHeight - 8) {
        top = Math.max(8, rect.top - pop.offsetHeight - 6);
      }
      pop.style.left = left + "px";
      pop.style.top = top + "px";
    }

    function open(key, anchor) {
      if (!index.has(key)) return;
      current = key;
      ensurePop();
      render();
      pop.hidden = false;
      place(anchor);
    }

    function close() {
      if (!pop || pop.hidden) return;
      pop.hidden = true;
      current = null;
    }

    /// 浮层开着时按新语言重画(`R.applyLang` 会调)。
    function refresh() {
      if (pop && !pop.hidden) render();
    }

    /// 把 root 里出现的术语包成可点词条。
    ///
    /// 默认**跳过 `pre` / `code`**: AI 气泡里的代码块是标识符或字面量, 挂术语会误导(044)。
    /// 但回测的"完整文本报告"整体在 `<pre>` 里 —— 那是**报告**不是代码, 故视图可显式
    /// `decorate(pre, { allowPre: true })` 打开。
    ///
    /// 术语表还没取到时先记下, 取回后若这块 DOM 仍在文档里就补一次(视图换掉就不补)。
    function decorate(root, opts) {
      if (!root) return;
      if (!re) {
        load()
          .then(() => {
            if (root.isConnected) decorate(root, opts);
          })
          .catch(() => {});
        return;
      }
      const skip = opts && opts.allowPre ? "code" : "pre, code";
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      const nodes = [];
      while (walker.nextNode()) nodes.push(walker.currentNode);
      for (const node of nodes) {
        const parent = node.parentElement;
        if (!parent) continue;
        if (parent.classList.contains("term") || parent.closest(skip)) continue;
        const raw = node.nodeValue;
        re.lastIndex = 0;
        let matched = re.exec(raw);
        if (!matched) continue;
        const frag = document.createDocumentFragment();
        let last = 0;
        while (matched) {
          if (matched.index > last) {
            frag.appendChild(document.createTextNode(raw.slice(last, matched.index)));
          }
          frag.appendChild(button(matched[0]));
          last = matched.index + matched[0].length;
          matched = re.exec(raw);
        }
        if (last < raw.length) frag.appendChild(document.createTextNode(raw.slice(last)));
        node.parentNode.replaceChild(frag, node);
      }
    }

    return { load, has, all, text, button, decorate, close, refresh };
  })();
})();
