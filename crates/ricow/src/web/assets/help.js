// Ricow Web 前端「帮助」层: 顶栏常驻 `?` 打开的**按当前视图**组织的帮助抽屉, 以及只对
// 全新装置出现一次的上手引导条(B5)。两者共用同一份 GUIDE 数据 —— 不各写一份"上手步骤"。
//
// 为什么是抽屉不是第 7 个导航项: 导航项是"目的地", 帮助是"随时能盖上来的一层"。用户在策略页
// 卡住时不该先离开策略页。抽屉挂在 `#app` 下, 由 `hidden` 控制显隐, **不参与 hash 路由**。
//
// ── 单一真相纪律(本文件最重要的一条) ─────────────────────────────────────────────
// 后端的能力清单 / 命令表**只有一份**: `ai/session.rs` 的 `help_text()`。本抽屉**不抄第二份**,
// 只放一个「看看 AI 能做什么」按钮 —— 点了跳到对话并把 `/help` 填进输入框, 由后端回答。
// 前端自有文案仅限三块, 且都不与后端数字/清单重叠:
//   ① 上手步骤(GUIDE)  ② 常见问题(FAQ)  ③ 术语表(走公共件的全局单例 `R.glossary`,
//   它的源头也是接口 `/api/terms` —— 前端只此一处取, 各视图共享同一份索引)
// 一旦这里开始复述"需要确认的动作有几类""一共有几个工具"这类**别人维护的数字**, 就是第二份
// 真相: 改一处漏一处, 而且假得看不出来。
//
// 安全: 不写浏览器存储; 服务端文本(术语解释)一律 textContent; 无内联事件(全部 addEventListener)。
// 说明: 本文件与 chat.js 共用 `R.TEXT` 单一文案字典, 不自立第二份。
"use strict";

(function () {
  const R = window.Ricow;

  // ---------- 文案(并入公共字典; 静态标签走 data-zh/data-en) ----------
  Object.assign(R.TEXT.zh, {
    helpTitle: "帮助",
    helpIntro:
      "随时点顶栏这个按钮回来。也可以直接问 —— 左侧「对话」里用大白话就行, 不必记命令。",
    helpStart: "5 分钟上手",
    helpTalk: "怎么跟我说话",
    helpTalkLead: "用大白话直接说, 不必记命令。例如:",
    helpAskCap: "看看 AI 能做什么",
    helpFaq: "常见问题",
    helpTerms: "术语速查",
    helpTermsPh: "搜索术语…",
    helpTermsEmpty: "没有匹配的术语。",
    onboardLead: "看起来是第一次用 —— 几步就能让它跑起来:",
    onboardStart: "从配密钥开始",
    onboardGuide: "看完整上手",
    onboardClose: "知道了, 先收起",
  });
  Object.assign(R.TEXT.en, {
    helpTitle: "Help",
    helpIntro:
      "Click this button any time to come back. You can also just ask — plain language works in the Chat view; no commands to memorise.",
    helpStart: "Get started in 5 minutes",
    helpTalk: "How to talk to me",
    helpTalkLead: "Just say it in plain language — no commands to memorise. For example:",
    helpAskCap: "See what the AI can do",
    helpFaq: "FAQ",
    helpTerms: "Glossary",
    helpTermsPh: "Search terms…",
    helpTermsEmpty: "No matching term.",
    onboardLead: "Looks like a fresh setup — a few steps and it's running:",
    onboardStart: "Start with keys",
    onboardGuide: "Full 5-minute guide",
    onboardClose: "Got it — hide this",
  });

  const t = (key) => R.t(key);

  // ---------- 前端自有内容(不与后端 help_text 重叠, 见文件头纪律) ----------

  const GUIDE = {
    zh: [
      {
        h: "配密钥",
        p: "左侧「密钥」页保存币安凭据, 以及(可选)AI 通道的 API Key。值只写进本机 ricow.toml。",
      },
      {
        h: "选策略",
        p: "「策略」页点开任一策略的「详情」, 先看它的**适用 / 不适用场景**, 再决定要不要用。要改成自己的就用列表里的「复制」—— 参数表会一起复制过来; 参数表空着就点「从脚本识别参数」。AI 修改框默认只改参数, 不动代码。",
      },
      {
        h: "先回测",
        p: "同一个详情页往下就是回测。它只读本地历史 K 线, 不碰资金 —— 放心多跑几组参数。",
      },
      {
        h: "再试跑",
        p: "「运行」页用 Dry Run(虚拟撮合)在真实行情里观察它怎么动, 同样不用真钱。",
      },
      {
        h: "最后实盘",
        p: "实盘要先逐字确认风险披露。撤单 / 平仓 / 停机都在「对话」里说一句, 你确认后才由本机执行。",
      },
    ],
    en: [
      {
        h: "Add keys",
        p: "On the Keys page, save your Binance credentials and (optionally) the AI channel's API key. Values go only into your local ricow.toml.",
      },
      {
        h: "Pick a strategy",
        p: "On the Strategies page, open any strategy's Details and read its **good for / not suitable for** notes before choosing. To make it your own, use Copy in the list — the parameter form comes with it; if it is empty, click “Detect parameters from script”. The AI box tunes parameters by default and leaves the code alone.",
      },
      {
        h: "Backtest first",
        p: "The same detail page has the backtest below. It only reads local historical candles and never touches funds — run as many parameter sets as you like.",
      },
      {
        h: "Then dry run",
        p: "On the Runs page, use Dry Run (simulated fills) to watch it behave on live prices, still with no real money.",
      },
      {
        h: "Go live last",
        p: "Going live requires typing the risk acknowledgement. Cancel / close / stop are all said in Chat and only run after you confirm.",
      },
    ],
  };

  const TALK = {
    zh: [
      { q: "我部署了哪些策略?", p: "看现状" },
      { q: "把 shannon_grid 的网格间距改成 0.006", p: "改参数(先给你看改前改后)" },
      { q: "回测 shannon_grid 最近 90 天", p: "跑一次回测" },
      { q: "给 SOLUSDT 写一个 50:50 再平衡策略", p: "从想法到代码" },
    ],
    en: [
      { q: "Which strategies do I have deployed?", p: "See what's there" },
      { q: "Change shannon_grid's grid spacing to 0.006", p: "Update a parameter (before/after shown first)" },
      { q: "Backtest shannon_grid over the last 90 days", p: "Run a backtest" },
      { q: "Write a 50:50 rebalance strategy for SOLUSDT", p: "Idea to code" },
    ],
  };

  // 常见问题: 只写"该往哪看/为什么这样设计", 不写会过期的数字与清单。
  const FAQ = {
    zh: [
      // 054 T39: 「平台不代做风控」的**用户可读版本**。
      // 给模型看的是 `ai::prompt::TRAPS_GUIDE`(那条是命令式核对清单, 语气是"逐条核对"),
      // 这里要说人话: 用户真正想知道的是"那谁来止损"以及"我该怎么确认"。
      {
        q: "平台会替我止损、替我控风险吗?",
        a: "不会。ricow 不代做盈亏风控: 亏损熔断、峰值回撤止损都要策略自己实现(在策略里读 ctx:net_pnl() / ctx:equity() 自己判断)。平台只留工程护栏 —— 比如下单频率上限, 那是防程序失控, 不是防亏钱。保存策略时如果脚本里看不到任何止损或仓位上限的线索, 会提示一句; 只提示, 拦不拦由你决定。",
      },
      {
        q: "回测一笔都没成交, 为什么?",
        a: "最常见是必填参数没填(例如香农的「开始价格」)、预热 K 线根数不够, 或开仓条件始终没触发。打开详情页展开「策略日志」, 能看到策略自己报出的原因。",
      },
      {
        q: "为什么启动被拒绝?",
        a: "两类会被拦: 只放了 .lua 却没写参数清单的「裸脚本」; 以及与某个内置脚本逐字相同的副本。详情页顶部会用红色提示条说明是哪种、怎么改。",
      },
      {
        q: "为什么页面上不能直接下单 / 撤单?",
        a: "这是刻意的: 本机网页不直连交易所, 避免误点。撤单 / 平仓 / 停机都在「对话」里说一句, 你确认后才由本机执行。",
      },
      {
        q: "密钥存在哪里?",
        a: "只写进本机 ricow.toml —— 不经浏览器存储、不进对话流, 页面上也没有任何上传口。",
      },
      {
        q: "改了策略需要重启吗?",
        a: "实例运行中不允许保存改动(保存钮会被锁)。改参数会按原模式自动重启, 但同样需要你确认。",
      },
    ],
    en: [
      {
        q: "Does the platform stop my losses or manage risk for me?",
        a: "No. ricow does not do profit-and-loss risk control for you: loss circuit-breakers and peak-drawdown stops have to be implemented inside the strategy itself (read ctx:net_pnl() / ctx:equity() and decide there). What the platform keeps are engineering guardrails — e.g. an order-rate cap — which protect against a runaway program, not against losing money. When you save a strategy, if the script shows no trace of a stop-loss or a position cap, you get one hint. It is a hint only; blocking is your call.",
      },
      {
        q: "My backtest made no trades at all — why?",
        a: "Usually a required parameter is missing (e.g. Shannon's start price), there aren't enough warm-up candles, or the entry condition never triggered. Open the detail page and expand Strategy logs to see the strategy's own reason.",
      },
      {
        q: "Why was my start refused?",
        a: "Two cases are blocked: a bare .lua with no parameter manifest, and a copy that is byte-identical to a built-in script. The detail page shows a red notice saying which one it is and how to fix it.",
      },
      {
        q: "Why can't I place or cancel orders in the page?",
        a: "By design: the local web page never talks to the exchange directly, so a stray click can't move funds. Cancel / close / stop are said in Chat and only executed locally after you confirm.",
      },
      {
        q: "Where are my keys stored?",
        a: "Only in your local ricow.toml — not in browser storage, not in the chat stream, and there is no upload field anywhere on the page.",
      },
      {
        q: "Do I need to restart after changing a strategy?",
        a: "Saving is blocked while the instance is running (the save button locks). Changing parameters restarts it in the same mode, and that still needs your confirmation.",
      },
    ],
  };

  /// 当前视图 → 打开时高亮的那一段(帮助"先答你在哪")。
  /// 062: 加 `dashboard`(概览)→ 落在"术语"段(概览上的数字与链接多指向术语); `keys`/`settings`
  /// 原本没有对应段, 保持"不高亮任何段"(不硬凑)。
  const CONTEXT = {
    strategies: "start",
    runs: "faq",
    chat: "talk",
    markets: "terms",
    dashboard: "terms",
  };

  // ---------- DOM ----------
  const $ = (id) => document.getElementById(id);

  function h(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  /// 清空一个节点: 逐个子节点 `removeChild`。
  /// **不靠给节点赋一段 HTML 字符串来清空** —— 与 palette.js / markdown.js 同一条纪律:
  /// 本层不经手 HTML 字符串, 清空也走 DOM API, 免得给"这文件到底拼不拼 HTML"留下中间态。
  function clear(node) {
    while (node.firstChild) node.removeChild(node.firstChild);
  }

  function section(host, id, titleKey) {
    const sec = h("section", "help-sec");
    sec.id = "help-sec-" + id;
    const head = h("h3", "help-sec-title", t(titleKey));
    head.setAttribute("data-zh", R.TEXT.zh[titleKey]);
    head.setAttribute("data-en", R.TEXT.en[titleKey]);
    sec.appendChild(head);
    host.appendChild(sec);
    return sec;
  }

  /// 术语速查: 输入框 + 列表; 列表随输入即时过滤(中英都按当前语言匹配)。
  ///
  /// 数据来自公共件的全局术语单例(`R.glossary`) —— **前端只此一处** 取 `/api/terms`,
  /// 抽屉与对话 / 回测 / 市场页共享同一份索引, 而不是各取一份(取两份就会出现"抽屉里搜得到,
  /// 页面上点不动"这种说不清的状态)。
  ///
  /// 输入框的 `type` 用 `setAttribute` 设 —— 直接写属性赋值会被纪律扫描误判(它禁的是
  /// `<textarea>` 的 type 赋值, 但扫描是字面量匹配)。
  function renderTerms(sec) {
    const box = h("input", "help-terms-input");
    box.setAttribute("type", "search");
    box.setAttribute("spellcheck", "false");
    box.setAttribute("autocomplete", "off");
    box.placeholder = t("helpTermsPh");
    box.setAttribute("data-zh-placeholder", R.TEXT.zh.helpTermsPh);
    box.setAttribute("data-en-placeholder", R.TEXT.en.helpTermsPh);
    const list = h("div", "help-terms-list");
    const empty = h("div", "help-terms-empty muted", t("helpTermsEmpty"));
    empty.hidden = true;
    sec.appendChild(box);
    sec.appendChild(list);
    sec.appendChild(empty);

    function paint() {
      const q = box.value.trim().toLowerCase();
      clear(list);
      const lang = R.lang;
      let shown = 0;
      for (const term of R.glossary.all()) {
        const text = lang === "en" ? term.en : term.zh;
        if (q && !(term.key.toLowerCase().includes(q) || text.toLowerCase().includes(q))) {
          continue;
        }
        shown += 1;
        const row = h("div", "help-term");
        // 服务端文本只走 textContent(见 h())。
        row.appendChild(h("div", "help-term-key", term.key));
        row.appendChild(h("div", "help-term-text", text));
        list.appendChild(row);
      }
      empty.hidden = shown > 0;
    }

    box.addEventListener("input", paint);
    paint();
    // 术语表取不到就留空列表 —— **不编解释**(诚实性三则同款: 读不到就说读不到)。
    R.glossary
      .load()
      .then(paint)
      .catch(() => {
        empty.textContent = t("helpTermsEmpty");
        empty.hidden = false;
      });
  }

  /// 整卡重建: 每次打开都按**当前语言与当前视图**重画 —— 避免"语言切了抽屉还是旧文本"
  /// 这类要跨模块刷新才能修的耦合。
  function build() {
    const body = $("help-body");
    clear(body);
    $("help-title").textContent = t("helpTitle");

    body.appendChild(h("p", "help-intro", t("helpIntro")));

    // ① 5 分钟上手
    const start = section(body, "start", "helpStart");
    const ol = h("ol", "help-guide");
    for (const step of GUIDE[R.lang] || GUIDE.zh) {
      const li = h("li");
      li.appendChild(h("span", "help-guide-h", step.h));
      li.appendChild(h("span", "help-guide-p", step.p));
      ol.appendChild(li);
    }
    start.appendChild(ol);

    // ② 怎么跟我说话
    const talk = section(body, "talk", "helpTalk");
    talk.appendChild(h("p", "help-lead", t("helpTalkLead")));
    const ul = h("ul", "help-examples");
    for (const ex of TALK[R.lang] || TALK.zh) {
      const li = h("li");
      const say = h("span", "help-say", ex.q);
      li.appendChild(say);
      li.appendChild(h("span", "help-say-note muted", ex.p));
      ul.appendChild(li);
    }
    talk.appendChild(ul);
    // 能力清单**不在这里复述**: 点一下去对话里让后端答(见文件头"单一真相纪律")。
    const capBtn = h("button", "help-btn-primary", t("helpAskCap"));
    capBtn.type = "button";
    capBtn.addEventListener("click", askCapabilities);
    talk.appendChild(capBtn);

    // ③ 常见问题
    const faq = section(body, "faq", "helpFaq");
    for (const item of FAQ[R.lang] || FAQ.zh) {
      const row = h("div", "help-faq");
      row.appendChild(h("div", "help-faq-q", item.q));
      row.appendChild(h("div", "help-faq-a", item.a));
      faq.appendChild(row);
    }

    // ④ 术语速查
    const terms = section(body, "terms", "helpTerms");
    renderTerms(terms);

    const ctx = CONTEXT[currentViewName()];
    body.scrollTop = 0;
    if (ctx) {
      const target = $("help-sec-" + ctx);
      if (target) {
        target.classList.add("help-ctx");
        target.scrollIntoView({ block: "start" });
      }
    }
  }

  /// 「看看 AI 能做什么」: 关抽屉 → 去对话 → 把 `/help` 填进输入框。
  /// **不代替用户发送**: 直接把内容塞进去并聚焦, 由用户按 Enter —— 避免与 boot 的
  /// "打开最近会话"竞态(发到一个马上被切走的会话里, 用户会以为没反应)。
  function askCapabilities() {
    close();
    R.navigate("chat");
    const input = $("input");
    if (input) {
      input.value = "/help";
      input.focus();
    }
  }

  function isOpen() {
    const drawer = $("help-drawer");
    return !!drawer && !drawer.hidden;
  }

  function open(section) {
    const drawer = $("help-drawer");
    if (!drawer) return;
    build();
    drawer.hidden = false;
    document.body.classList.add("help-open");
    // B7: 命令面板可以指定"打开并滚到某一段"(`start` / `talk` / `faq` / `terms`)。
    // 不传就沿用按当前视图高亮的老行为(build 里已处理)。
    if (section) {
      const target = $("help-sec-" + section);
      if (target) {
        for (const sec of document.querySelectorAll(".help-sec")) sec.classList.remove("help-ctx");
        target.classList.add("help-ctx");
        target.scrollIntoView({ block: "start" });
      }
    }
    const closeBtn = $("help-close");
    if (closeBtn) closeBtn.focus();
  }

  function close() {
    const drawer = $("help-drawer");
    if (!drawer) return;
    drawer.hidden = true;
    document.body.classList.remove("help-open");
  }

  // ---------- 首次引导条(B5) ----------
  //
  // 只在**真正全新**的装置上出现: 零密钥 && 零自有策略 && 零实例 —— 三条全是**服务端事实**
  // (三个 GET 各探一次)。既不落浏览器存储(纪律禁), 也不靠"是不是第一次打开"这种只能猜出来
  // 的状态: 那个状态一旦写错, 用户会看到"你还没配密钥"而其实早配好了。
  //
  // ★ 为什么**不把"零会话"当判据**: 对话视图一启动就会自建一个会话(chat.js 的 boot), 于是
  //   "到底有没有会话"取决于探针与建会话谁先跑 —— 是个竞态。曾经的实现把 `sessions.length === 0`
  //   也算一条, 结果首屏偶尔能看到引导条, 而任意一次 hashchange 触发重探时会话已 ≥1, 条子就自己
  //   消失了(慢机上甚至从不出现)。它根本不是"用户起步了"的可靠信号, 去掉它之后判据全是**稳定**
  //   事实: 只要还没配密钥 / 还没自建策略 / 还没跑过实例, 条子就一直在 —— 用户做完其中任一步才消失。
  //
  // 内容**不另写一份**: 条上就是抽屉里 GUIDE 的那几步(同一份数据), 点开看全套。两处各写一份
  // "上手步骤"迟早对不上 —— 与文件头那条单一真相纪律同一个理由。
  //
  // ★ 探针**任何一个** GET 失败就闭嘴(既不报错也不引导): "读不到"不等于"还没开始用"。
  let onboardDismissed = false; // 本次页面生命周期内点过"知道了"; 刷新按服务端事实重算

  async function probeOnboard() {
    const bar = $("onboard-bar");
    if (!bar || onboardDismissed) return;
    let keys;
    let strategies;
    let runs;
    try {
      [keys, strategies, runs] = await Promise.all([
        R.api("/api/keys"),
        R.api("/api/strategies"),
        R.api("/api/runs"),
      ]);
    } catch (_) {
      bar.hidden = true;
      return;
    }
    // 密钥: 主网与测试网任一配好都算"起步了"(演练不需要实盘凭据)。
    const liveReady = !!(keys && keys.binance_key && keys.binance_key.configured);
    const demoReady = !!(keys && keys.demo_key && keys.demo_key.configured);
    const fresh =
      !liveReady &&
      !demoReady &&
      !strategies.some((s) => s.source === "user") &&
      runs.length === 0;
    if (!fresh) {
      bar.hidden = true;
      return;
    }
    renderOnboard(bar);
  }

  function renderOnboard(bar) {
    clear(bar);
    bar.appendChild(h("div", "onboard-lead", t("onboardLead")));
    const ol = h("ol", "onboard-steps");
    for (const step of GUIDE[R.lang] || GUIDE.zh) {
      ol.appendChild(h("li", null, step.h));
    }
    bar.appendChild(ol);
    const actions = h("div", "onboard-actions");
    const go = h("button", "onboard-btn", t("onboardStart"));
    go.type = "button";
    go.addEventListener("click", () => R.navigate("keys"));
    actions.appendChild(go);
    const more = h("button", "onboard-btn onboard-ghost", t("onboardGuide"));
    more.type = "button";
    more.addEventListener("click", () => open());
    actions.appendChild(more);
    bar.appendChild(actions);
    const closeBtn = h("button", "onboard-close", "\u00d7");
    closeBtn.type = "button";
    closeBtn.title = t("onboardClose");
    closeBtn.setAttribute("aria-label", t("onboardClose"));
    closeBtn.addEventListener("click", () => {
      // 只记在内存里: 用户一旦真去配了密钥, 上面的服务端事实就会让它**自己**消失,
      // 不需要我们再维护一个"已看过引导"的开关(那是第二处状态, 迟早不同步)。
      onboardDismissed = true;
      bar.hidden = true;
    });
    bar.appendChild(closeBtn);
    bar.hidden = false;
  }

  function wire() {
    const btn = $("help-btn");
    const overlay = $("help-overlay");
    const closeBtn = $("help-close");
    if (btn) btn.addEventListener("click", () => (isOpen() ? close() : open()));
    if (overlay) overlay.addEventListener("click", close);
    if (closeBtn) closeBtn.addEventListener("click", close);
    document.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape" && isOpen()) close();
    });
    // 切视图时收起抽屉: 它是"盖在当前页上的一层", 页面换了就不该继续盖着。
    window.addEventListener("hashchange", () => {
      if (isOpen()) close();
    });
    // B5: 首次引导条探一次; 它还挂着时跟着切视图重探 —— 用户做完一步(比如配好密钥),
    // 条该自己消失, 而不是等他刷新。
    probeOnboard();
    window.addEventListener("hashchange", () => {
      const bar = $("onboard-bar");
      if (bar && !bar.hidden && !onboardDismissed) probeOnboard();
    });
  }

  /// 当前一级视图名: **读 `router.js` 维护的唯一真相** `R.currentView`(051 复核 B12)。
  /// 早先这里另有一份从 `location.hash` 抠首段的副本, 与路由的校正(replaceState 回退 #chat)
  /// 不同步 —— 非法 hash 首屏会让帮助抽屉的「当前视图」高亮错位。现在只读不写。
  function currentViewName() {
    return R.currentView || "chat";
  }

  R.help = { open: open, close: close };

  // 语言切换时**就地重画**: 抽屉内容是打开时按语言构建的(不走 data-zh 那套静态标签刷新),
  // 不重画就会留下"切了 EN 抽屉还是中文"。与 strategies.js 同一手法(包一层 applyLang)。
  const baseApplyLang = R.applyLang;
  R.applyLang = function (lang) {
    const ret = baseApplyLang.call(this, lang);
    try {
      if (isOpen()) build();
      // 引导条也按新语言重画(它与抽屉共用 GUIDE, 但渲染是分开的)。
      const bar = $("onboard-bar");
      if (bar && !bar.hidden) renderOnboard(bar);
    } catch (_) {
      // 抽屉骨架不在(理论上不会)时忽略, 不影响语言切换本身。
    }
    return ret;
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", wire);
  } else {
    wire();
  }
  // 051 复核 B12: 不再在这里监听 hashchange 维护副本 —— `R.currentView` 由 router.js 独家维护。
})();
