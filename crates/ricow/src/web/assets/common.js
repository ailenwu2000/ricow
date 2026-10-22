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
  /// 059 UX-05: 可选的**前端超时**(`opts.timeoutMs`, 毫秒)。默认**不启用** —— 既有调用点
  /// 行为不变(回测作业等本来就靠服务端 202 + 轮询, 不该被前端掐表)。
  /// 要它的理由: `fetch` 自己没有超时, 上游一直不回就会**永远停在「加载中…」** ——
  /// 用户分不清是网络慢、服务没起还是软件卡死, 也没有任何可点的东西。调用方显式传一个
  /// 上限, 到点就把「加载中」换成「失败 + 重试」(错误盒由各视图自己渲染)。
  R.api = async function (path, options) {
    const opts = Object.assign({}, options);
    const timeoutMs = Number(opts.timeoutMs) || 0;
    delete opts.timeoutMs; // 不是 fetch 的选项, 别带进请求里
    opts.headers = Object.assign({ Authorization: "Bearer " + R.TOKEN }, opts.headers || {});
    if (opts.body !== undefined) {
      opts.headers["Content-Type"] = "application/json";
      opts.body = JSON.stringify(opts.body);
    }
    let res;
    if (timeoutMs > 0) {
      const ac = new AbortController();
      opts.signal = ac.signal;
      const timer = setTimeout(() => ac.abort(), timeoutMs);
      try {
        res = await fetch(path, opts);
      } catch (err) {
        // abort 抛的是浏览器原生的 AbortError, 原文("The user aborted a request.")
        // 摆在界面上是句没用的话 —— 换成"超时"这个**真实原因**。
        if (err && err.name === "AbortError") {
          const e = new Error(
            (R.lang === "en" ? "Timed out after " : "请求超时(超过 ") +
              Math.round(timeoutMs / 1000) +
              (R.lang === "en" ? "s with no response)" : " 秒无响应)")
          );
          e.timeout = true;
          throw e;
        }
        throw err;
      } finally {
        clearTimeout(timer); // 成功了也要清, 否则定时器白挂着
      }
    } else {
      res = await fetch(path, opts);
    }
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
  /// **连上并稳定存活 `STABLE_MS` 才算健康**并清零重计(见下)。返回 `{ close() }`。
  R.sse = function (url, handlers) {
    const MAX_RETRIES = 5;
    // 051 复核 B5: 早先"一 `onopen` 就清零"会把**半死服务端**(握手 200 后立即 EOF)变成
    // 永久 ~1s 忙重连 —— 每轮都触发 `onopen` 清零, 退避永远封不了顶、`onfail` 也永不回调。
    // 改为"连上并**稳定存活** `STABLE_MS` 才算一次成功连接": 稳定期内掉线继续累计 attempts
    // (退避照常封顶到 `onfail` 止损)。判据用**时间**而非"收到消息": 盘口/日志流可能长时间
    // 静默但连接是健康的, 拿"有没有消息"当健康信号会误杀静默流。
    const STABLE_MS = 10000;
    let es = null;
    let attempts = 0;
    let lastId = null;
    let timer = null;
    let stableTimer = null;
    let closed = false;

    function clearStable() {
      if (stableTimer) {
        clearTimeout(stableTimer);
        stableTimer = null;
      }
    }

    function open() {
      if (closed) return;
      const target =
        lastId === null ? url : url + "&last_event_id=" + encodeURIComponent(lastId);
      es = new EventSource(target);
      es.onopen = () => {
        clearStable();
        stableTimer = setTimeout(() => {
          attempts = 0;
        }, STABLE_MS);
        if (handlers.onopen) handlers.onopen();
      };
      es.onmessage = (ev) => {
        if (ev.lastEventId) lastId = ev.lastEventId;
        if (handlers.onmessage) handlers.onmessage(ev);
      };
      es.onerror = () => {
        clearStable();
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
        clearStable();
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
    // 059 UX-01/02: 顶栏视图名与浏览器标签标题取自导航项文本, 切语言后要跟着重刷 ——
    // 否则会停在切换前那一门语言的视图名上。
    if (R.refreshTitle) R.refreshTitle();
  };

  // ---------- 纯工具 ----------

  // 策略详情参数归一化: 清单 JSON 里类型字段名是 `type`(后端 serde rename), 而前端渲染链
  // (`renderParams` / 清单编辑器 / 模板表单)统一读 `p.ty` —— 不补这一笔, string/bool/enum
  // 会全部退化成数字输入框, 且清单编辑器保存时会把真实类型拍平成 f64。
  // 就地补 `ty` 并返回 detail, 在每个 `/api/strategies/<id>` 取回点调用一次。
  R.normParams = function (detail) {
    if (!detail || !Array.isArray(detail.params)) return detail;
    for (const p of detail.params) {
      if (p && p.ty === undefined && typeof p.type === "string") p.ty = p.type;
    }
    return detail;
  };

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
    /// 去尾零(**纯字符串, 不经过 `Number`**): `"0.30000000"` → `"0.3"`、`"100.00000000"` → `"100"`、
    /// `"0.00000000"` → `"0"`; 无小数点(含 `1e+21` 这类指数形态)原样返回。
    function stripTrailingZeros(s) {
      if (s.indexOf(".") < 0) return s;
      let out = s.replace(/0+$/, "");
      if (out.endsWith(".")) out = out.slice(0, -1);
      return out === "" || out === "-" ? "0" : out;
    }
    /// 本地化日期串; 超出 `Date` 可表示范围时返回 `null`(调用方按"缺值"处理, 绝不吐 "Invalid Date")。
    function localStamp(ms, opts) {
      const d = new Date(ms);
      if (isNaN(d.getTime())) return null;
      return d.toLocaleString(locale(), opts);
    }
    return {
      /// 无符号数字: 按 `digits` 位截断再去尾零(`0.30000000000000004` → `0.3`)。
      /// 缺值 / 非有限 → `—`(绝不把 NaN / Infinity 显示出去)。
      num: function (v, digits) {
        const n = Number(v);
        if (v == null || !Number.isFinite(n)) return "—";
        const d = typeof digits === "number" ? digits : 8;
        // 051 复核 B2: 早先 `String(Number(n.toFixed(d)))` 的往返会把 `1e-7` 这样的**小数**
        // 变回科学计数法 `"1e-7"`(低价币现价/盘口会直接把它画进界面)。改为与 `signed` 对齐的
        // **纯字符串去尾零**, 不经过 `Number` 往返。
        return stripTrailingZeros(n.toFixed(d));
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
        // 051 复核 B3: 有限但超 `Date` 范围(如 `1e300`)会得到 Invalid Date, 而
        // `toLocaleString` 对它返回字面量 `"Invalid Date"` —— 契约是"缺值给 —", 这里补上。
        const s = localStamp(ms, withSeconds ? stampOptsSec : stampOpts);
        return s === null ? "—" : s;
      },
      /// 秒级时间戳 → 本地化展示串(会话列表的 `updated_at`)。缺值 → `""`。
      timeSec: function (ts) {
        // 051 复核 B3: 早先 `if (!ts) return "";` 之后直接乘 1000 —— `"abc"` 是 truthy,
        // `"abc" * 1000 = NaN` → `"Invalid Date"`。改为先做数值合法性判定(仍保留 0/""/null
        // 这些 falsy 值 → `""` 的既有语义)。
        if (!ts) return "";
        const n = Number(ts);
        if (!Number.isFinite(n)) return "";
        const s = localStamp(n * 1000, stampOpts);
        return s === null ? "" : s;
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
  //
  // 051 复核 B4 —— **唯一例外, 且只有一处**: 发起动作的控件是"**点完即关的一次性浮层**"
  // (命令面板 `palette.js`)时, 动作失败时浮层已收起、**没有原位可贴** —— 该处错误退回本通知
  // (`kind="err"`), 因为"完全没有提示"比"提示位置不理想"更糟。除此之外一律原地显示;
  // 这条例外由 `scripts/web_frontend_check.mjs` 的静态检查锁住(只允许 palette.js 用 err toast),
  // 防止它悄悄扩散到别的视图。
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

  // ---------- 弹窗(全站唯一实现) ----------
  //
  // 059 UX-09: 此前 `strategies.js` 与 `runs.js` **各写一份** `openModal` / `modalActions`
  // (除挂载点 `sg-modal-root` / `rn-modal-root` 外逐字相同), 且两份都缺三样东西:
  //   ① 没有 `role="dialog"` / `aria-modal` —— 读屏软件读不出这是个对话框;
  //   ② 打开后焦点不进弹窗, `Tab` 会跑到背后页面去, 键盘用户点不到弹窗里的按钮;
  //   ③ `Esc` 的 `keydown` 监听**只在 Esc 分支里移除** —— 点「取消」或主钮关闭时监听
  //      留在 `document` 上不动, 一次弹窗留一个, 越积越多。
  // 现在实现收在这里(挂载点 `#modal-root` 见 index.html), 两个视图只管往 `card` 里放内容。
  //
  // 与 `R.emptyState` 同姿态: 只管"壳", 不替调用方决定内容; 关闭一律走返回的 `close()`。
  R.modal = (function () {
    /// 可聚焦元素(够用即可: 弹窗里只有按钮 / 输入框 / 下拉 / 链接)。
    const FOCUSABLE =
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
    let seq = 0;
    const stack = []; // 已开的弹窗各自的 close(), 供 closeAll / open 前清场逐个调用

    function host() {
      let el = document.getElementById("modal-root");
      if (!el) {
        // 兜底: 骨架缺失时自建, 免得弹窗直接丢失(比崩一个 ReferenceError 好)。
        el = document.createElement("div");
        el.id = "modal-root";
        document.body.appendChild(el);
      }
      return el;
    }

    function focusables(card) {
      return Array.from(card.querySelectorAll(FOCUSABLE)).filter(
        (el) => el.offsetWidth > 0 || el.offsetHeight > 0 || el === document.activeElement
      );
    }

    function open(titleText) {
      // 关闭后把焦点还给"打开它的那个元素" —— 否则键盘用户关完弹窗焦点回到 body, 得重新 Tab。
      // 必须在清场**之前**取: 清场会连带把焦点还给上一个弹窗的触发元素。
      const restoreTo = document.activeElement;
      // ① 弹窗是单例 UI: 开新的必先清场。否则"上一个没关就又开一个"会在 `#modal-root`
      // 里摞好几层 dialog —— 059 排查时见过 3 层: 遮罩互相盖住、`querySelector` 取到的还是
      // 上一个弹窗的控件, 于是"输入框被清空/按钮放行"这类断言全成假象。
      closeAll();
      const overlay = document.createElement("div");
      overlay.className = "sg-modal-overlay";
      const card = document.createElement("div");
      card.className = "sg-modal-card";
      card.setAttribute("role", "dialog");
      card.setAttribute("aria-modal", "true");
      // 没有按钮时也要能被聚焦(否则 Tab 循环无处可去)。
      card.tabIndex = -1;
      const title = document.createElement("div");
      title.className = "sg-modal-title";
      title.textContent = titleText; // 标题也只走 textContent
      title.id = "modal-title-" + ++seq;
      card.setAttribute("aria-labelledby", title.id);
      card.appendChild(title);
      overlay.appendChild(card);
      host().appendChild(overlay);
      stack.push(close);

      // 关闭后把焦点还给"打开它的那个元素" —— 否则键盘用户关完弹窗焦点回到 body, 得重新 Tab。
      let onKey = null;

      function close() {
        // ③ 不论从哪条路径关闭(Esc / 遮罩 / 取消 / 主钮), 监听都在这里统一摘掉。
        if (onKey) {
          document.removeEventListener("keydown", onKey);
          onKey = null;
        }
        const at = stack.indexOf(close);
        if (at >= 0) stack.splice(at, 1);
        overlay.remove();
        if (restoreTo && document.contains(restoreTo) && typeof restoreTo.focus === "function") {
          try {
            restoreTo.focus();
          } catch (_) {
            /* 元素已不可聚焦时静默放过, 不因回焦失败影响关闭 */
          }
        }
      }

      overlay.addEventListener("mousedown", (ev) => {
        if (ev.target === overlay) close();
      });

      onKey = function (ev) {
        if (ev.key === "Escape") {
          close();
          return;
        }
        if (ev.key !== "Tab") return;
        // ② 焦点陷阱: Tab / Shift+Tab 只在弹窗内循环。
        const items = focusables(card);
        if (!items.length) {
          ev.preventDefault();
          card.focus();
          return;
        }
        const first = items[0];
        const last = items[items.length - 1];
        const active = document.activeElement;
        if (!card.contains(active)) {
          ev.preventDefault();
          first.focus();
        } else if (ev.shiftKey && active === first) {
          ev.preventDefault();
          last.focus();
        } else if (!ev.shiftKey && active === last) {
          ev.preventDefault();
          first.focus();
        }
      };
      document.addEventListener("keydown", onKey);

      // 打开后焦点进弹窗(等一帧, 让调用方先把内容塞进 card —— 那时才有可聚焦元素)。
      requestAnimationFrame(() => {
        const items = focusables(card);
        const target = items[0] || card;
        try {
          target.focus();
        } catch (_) {
          /* 同上 */
        }
      });

      return { card, close };
    }

    /// 弹窗底部一行: 取消 + 主钮; 返回 { cancel, primary }。文案走公共字典。
    function actions(card, primaryKeyOrLabel) {
      const row = document.createElement("div");
      row.className = "sg-modal-actions";
      const cancel = document.createElement("button");
      cancel.className = "sg-btn";
      cancel.type = "button";
      cancel.textContent = R.t("sgCancel");
      const primary = document.createElement("button");
      primary.className = "sg-btn sg-btn-primary";
      primary.type = "button";
      primary.textContent = R.t(primaryKeyOrLabel);
      row.appendChild(cancel);
      row.appendChild(primary);
      card.appendChild(row);
      return { cancel, primary };
    }

    /// 逐个关掉: 视图 deactivate 时清残留, 以及 `open()` 开新窗前清场。
    ///
    /// **不走 `innerHTML = ""`** —— 那样只摘 DOM, 每个弹窗自己挂在 document 上的 keydown
    /// 监听会留着(它们只在 `close()` 里摘), 关得越多漏得越多。逐个走 `close()` 才两清。
    function closeAll() {
      for (const c of stack.slice()) c();
    }

    return { open, actions, closeAll };
  })();

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
          // 051 复核 B6: **跳过空 key** —— 空串会进匹配式成为一个"零长分支",
          // 在任意位置匹配成功且不前进, 令 `decorate` 的扫描循环死循环。
          // 这里做第一道闸, `decorate` 里另有一道零长保护(双保险)。
          if (!term || typeof term.key !== "string" || term.key === "") continue;
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
          // 051 复核 B6: 零长匹配保护 —— 若匹配式里混进空分支(如空 key), `exec` 会在同一
          // 位置反复返回零长匹配, `last` 永不前进 → 死循环。手动推进 `lastIndex` 后继续。
          if (matched[0].length === 0) {
            re.lastIndex = matched.index + 1;
            matched = re.exec(raw);
            continue;
          }
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
