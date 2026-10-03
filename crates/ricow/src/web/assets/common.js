// Ricow Web 前端公共件 (032): 单命名空间 `window.Ricow` —— TOKEN / api / TEXT 字典 /
// 通用 i18n 与纯工具。032 从原 app.js 整体抽出, 逻辑零改写。
//
// 与 CLI 同源: 前端**不做任何业务判断**, 所有会话内容都经 `/api/**` + token 拿,
// 页面自身不落任何状态到磁盘。
"use strict";

(function () {
  const R = (window.Ricow = window.Ricow || {});

  // 页面 URL 上的一次性 token(与 `<script src="?token=">` 同一条取值路径, 见 web/mod.rs)。
  R.TOKEN = new URLSearchParams(location.search).get("token") || "";

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
