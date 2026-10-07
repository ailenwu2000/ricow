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
  };

  // ---------- 纯工具 ----------

  // ---------- 主题(034) ----------
  // 当前主题: 单一来源在 `ricow.toml` 的 `[ui].theme`(由 chat.js 的 boot 写入)。
  // CSS 侧 `:root` = 深色兜底, `[data-theme="light"]` / `[data-theme="red"]` 各覆盖一份变量;
  // 切换即换 `<html data-theme>`, 全站组件跟着变量走, 无需逐个重渲染。
  R.theme = "dark";

  R.applyTheme = function (theme) {
    R.theme = theme === "light" || theme === "red" ? theme : "dark";
    document.documentElement.dataset.theme = R.theme;
    const sel = document.getElementById("theme-select");
    if (sel) sel.value = R.theme;
    // 广播给关心外观的模块(如 markets.js 的 K 线图要按新变量重取配色)。
    window.dispatchEvent(new CustomEvent("ricow:theme", { detail: R.theme }));
  };

  /// 秒级时间戳(会话列表的 `updated_at`)→ 展示串。交易端点的毫秒级时间戳不用这个,
  /// 由对话视图自己的 `formatStamp` 处理(口径不同, 不能合并)。
  R.formatTime = function (ts) {
    if (!ts) return "";
    const locale = R.lang === "en" ? "en-US" : "zh-CN";
    return new Date(ts * 1000).toLocaleString(locale, {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  };
})();
