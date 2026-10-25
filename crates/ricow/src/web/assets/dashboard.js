// Ricow Web 前端「概览」视图 (062) —— fmz.com 的 **P1 仪表盘**范式。
//
// ── 为什么有这一页 ──────────────────────────────────────────────────────────────
// 062 把视觉基准换成 fmz.com 后, fmz 的"控制中心"是默认页 —— 一屏看清: 有多少策略、
// 有多少实例、有什么要处理的。其价值不在"好看", 在于**省掉一次次点开侧栏**。
// ricow 的对应物就是这一页: 概览。
//
// ── 与 fmz 的差别(刻意) ────────────────────────────────────────────────────────
// ① fmz 的概览右端有「账户卡」(余额/账户信息)与「消息中心卡」—— **ricow 无账号体系, 不做**,
//    那一块换成「运行状态」卡(实例数/最近成交/停止全部)。
// ② fmz 的统计数字**读不到就显示 0**; ricow 的诚实口径是**读不到就显示"—"**,
//    且失败要显示错误态 —— 把"没数据"说成"0"是假绿(见 plan.md 的假绿排查清单)。
// ③ fmz 的告警卡接的是站内消息; ricow 没有消息中心, 故这一块只放**回测任务的近期失败**,
//    读不到就不显示这一段(不硬凑)。
//
// ── 纪律 ────────────────────────────────────────────────────────────────────────
// ① **只产 DOM 节点**, 不拼 HTML 字符串(用户数据拼 HTML 是红线)。
// ② 数字**全部来自真实 API** —— 没有占位数字, 没有硬编码。
// ③ 本页**只读** —— 没有交易所写动作(与全站 D17 同一纪律)。
"use strict";

(function () {
  const R = window.Ricow;
  const t = (key) => R.t(key);

  /// 本页文案(并入公共字典, 与其它视图同手法)。
  Object.assign(R.TEXT.zh, {
    dashStats: "统计",
    dashStrategyCount: "策略",
    dashRunCount: "运行实例",
    dashKeyCount: "密钥",
    dashSessionCount: "会话",
    dashQuick: "快捷入口",
    dashQuickNewStrategy: "新建策略",
    dashQuickNewChat: "新建会话",
    dashQuickOpenMarkets: "打开市场",
    dashQuickOpenRuns: "打开运行",
    dashQuickOpenKeys: "打开密钥",
    dashQuickOpenStrategies: "打开策略",
    dashRunning: "运行状态",
    dashRunningCount: "运行中",
    dashTotal: "全部实例",
    dashNoRun: "当前没有实例。",
    dashHasRun: "实例清单在「运行」视图里。",
    dashDocs: "开发文档",
    dashDocsGlossary: "术语表",
    dashDocsGlossaryDesc: "回测、夏普、名义价值等术语解释",
    dashDocsUserGuide: "用户指南",
    dashDocsUserGuideDesc: "从零到跑起来的一步步",
    dashDocsLua: "Lua 策略语法",
    dashDocsLuaDesc: "内置变量、结构体、内置函数",
    dashDocsSafety: "密钥与安全",
    dashDocsSafetyDesc: "只读红线、权限边界",
    dashLoadFailed: "概览数据读取失败",
    dashRetry: "重试",
    // 064: bStock 覆盖卡(ricow 对 fmz 的差异化卖点)。
    dashBstock: "美股代币覆盖",
    dashBstockSpot: "现货",
    dashBstockFutures: "合约",
    dashBstockScopeStock: "股票类视野",
    dashBstockScopeAll: "全部交易对",
    dashBstockEdge:
      "币安官方的机器人只开放少数几个 stock; ricow 覆盖全部美股代币 —— 交易所上了哪个, 这里就能跑哪个。",
    dashBstockSeeAll: "去市场视图看全部",
    dashBstockUnknown: "视野规模暂不可读(交易所快照没拉到), 不影响其它数字。",
    // 066: 两侧里有一侧读不到时的说明(那一侧显示「—」, 不显示 0)。
    dashBstockPartial: "有一侧的视野读不到(交易所快照没拉到), 那一侧显示「—」。",
    // 064: 「最近操作」卡(fmz 的 dashboard 有同名的卡位, 我们填真实数据)。
    dashRecent: "最近操作",
    dashRecentEmpty: "还没有任何会话 —— 去「对话」提第一个需求吧。",
    dashRecentOpen: "打开会话",
    dashRecentUntitled: "(未命名会话)",
    dashRecentHint: "会话是 ricow 里唯一持久留痕的操作记录(回测结果只保留 5 分钟, 不作历史)。",
    // 064: 概览六格补链接(fmz 的六格每格都是可点的)。
    dashStatsStrategyGo: "去策略库",
    dashStatsRunGo: "去实盘",
    dashStatsKeyGo: "去密钥",
    dashStatsSessionGo: "去对话",
    // 064: 快捷入口补 AI 助手(fmz 六格里有)。
    dashQuickAiChat: "AI 助手",
    // 064: 告警格(fmz 概览六格之一是"告警消息")。
    dashAlert: "告警",
    dashAlertNone: "没有需要处理的策略问题。",
    dashAlertGo: "去处理",
    dashAlertBlocked: "被门禁拦住, 跑不起来",
  });
  Object.assign(R.TEXT.en, {
    dashStats: "Statistics",
    dashStrategyCount: "Strategies",
    dashRunCount: "Instances",
    dashKeyCount: "Keys",
    dashSessionCount: "Sessions",
    dashQuick: "Quick actions",
    dashQuickNewStrategy: "New strategy",
    dashQuickNewChat: "New session",
    dashQuickOpenMarkets: "Open markets",
    dashQuickOpenRuns: "Open runs",
    dashQuickOpenKeys: "Open keys",
    dashQuickOpenStrategies: "Open strategies",
    dashRunning: "Running",
    dashRunningCount: "Running now",
    dashTotal: "All instances",
    dashNoRun: "No instance right now.",
    dashHasRun: "See the instance list in the Runs view.",
    dashDocs: "Documentation",
    dashDocsGlossary: "Glossary",
    dashDocsGlossaryDesc: "Backtest, Sharpe, notional value and more",
    dashDocsUserGuide: "User guide",
    dashDocsUserGuideDesc: "Step by step from zero to running",
    dashDocsLua: "Lua strategy syntax",
    dashDocsLuaDesc: "Built-in variables, structs, functions",
    dashDocsSafety: "Keys and safety",
    dashDocsSafetyDesc: "Read-only red lines, permission boundaries",
    dashLoadFailed: "Failed to load dashboard data",
    dashRetry: "Retry",
    // 064: bStock coverage card (ricow's edge over fmz).
    dashBstock: "US equity tokens",
    dashBstockSpot: "Spot",
    dashBstockFutures: "Perps",
    dashBstockScopeStock: "Equity-only scope",
    dashBstockScopeAll: "All pairs",
    dashBstockEdge:
      "Binance's own bots only cover a handful of stocks; ricow covers all US equity tokens — whatever the exchange lists, you can trade here.",
    dashBstockSeeAll: "See all in the Markets view",
    dashBstockUnknown: "Scope size unavailable right now (exchange snapshot not fetched); other numbers are unaffected.",
    // 066: one side of the scope could not be read (that side shows "—", never 0).
    dashBstockPartial: "One side of the scope could not be read (exchange snapshot unavailable); that side shows —.",
    // 064: Recent activity card (fmz's dashboard has the same slot; we fill it with real data).
    dashRecent: "Recent activity",
    dashRecentEmpty: "No sessions yet — start one in the Chat view.",
    dashRecentOpen: "Open chat",
    dashRecentUntitled: "(untitled session)",
    dashRecentHint: "Sessions are the only operation record ricow keeps (backtest results live 5 minutes only, so they are not history).",
    // 064: stat cells get links (fmz's six cells are all clickable).
    dashStatsStrategyGo: "Go to strategies",
    dashStatsRunGo: "Go to runs",
    dashStatsKeyGo: "Go to keys",
    dashStatsSessionGo: "Go to chat",
    // 064: AI assistant quick entry (fmz has it in its six).
    dashQuickAiChat: "AI assistant",
    // 064: alert cell (fmz's six include "alert messages").
    dashAlert: "Alerts",
    dashAlertNone: "No strategy issues to handle.",
    dashAlertGo: "Handle it",
    dashAlertBlocked: "Blocked by the run gate",
  });

  function h(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined && text !== null) node.textContent = String(text);
    return node;
  }

  /// 本页状态。
  const st = {
    root: null,
    loading: false,
    /// 最近一次成功取到的数据(重渲染时复用, 避免每次切回来都闪一下空态)。
    data: null,
    /// 最近一次失败原因(如实展示, 不吞)。
    error: null,
  };

  /// 取数。**一次请求**拿全四项计数(`GET /api/overview`)。
  ///
  /// 062 修(两处坑, 都是真机撞出来的):
  ///
  /// ① 早先用 `Promise.all` 等四份全齐再一次性渲染 —— **任何一份慢, 整页就空着**,
  ///    且界面上分不清"还没加载完"与"没有数据"(都显示"—")。
  ///
  /// ② 更要紧的是**不能并发**。浏览器的同源 HTTP/1.1 连接数上限是 6, 而本页面**长期占着**:
  ///    会话 SSE 一条、交易面板每 5 秒轮询一次且**一次并发 3 路**(`/api/trades/*`)。
  ///    池满时新请求**排队等**, 等不到就超时 —— 实测概览四格一起显示"—",
  ///    而同样的接口用 curl 打是**毫秒级**返回(curl 不占浏览器的连接池)。
  ///    现在改走**后端聚合端点** `/api/overview`: 浏览器只占**一个**连接位,
  ///    服务端内部并行取四份(服务端没有浏览器的连接池限制)。
  ///
  /// 超时保留(8 秒)—— 免得真挂了把"加载中"变成永远。
  async function loadAll() {
    st.loading = true;
    st.error = null;
    let item;
    try {
      const v = await R.api("/api/overview", { timeoutMs: 8000 });
      item = { ok: true, value: v };
    } catch (err) {
      item = { ok: false, error: String((err && err.message) || err) };
    }
    st.data = item;
    st.loading = false;
    st.error = item.ok ? null : item.error;
    render();
  }

  /// 概览数据的四个计数(来自 `/api/overview`)。
  ///
  /// 062: 改走**后端聚合端点** —— 一次请求拿全, 只占浏览器一个连接位
  /// (原因见 `loadAll` 的说明: 页面常驻占着 SSE + 每 5 秒三路轮询, 连接池很紧)。
  ///
  /// `null` = **取不到**(显示"—"); `0` = **确实是 0 个**。两者必须能区分 ——
  /// 把"没数到"说成"0 个"是假绿。
  function counts(v) {
    const none = {
      strategies: null, runs: null, runsActive: null, keys: null, sessions: null,
      scope: null,
    };
    if (!v || !v.ok || !v.value) return none;
    const d = v.value;
    const num = (x) => (typeof x === "number" && Number.isFinite(x) ? x : null);
    // scope: 视野规模。**取不到(null)与 0 要分开** ——
    // 0 会是"交易所真的没有任何股票类交易对"(不可能, 但语义上要能区分)。
    const sc = d.scope;
    const scope =
      sc && typeof sc === "object" && num(sc.spot) !== null && num(sc.futures) !== null
        ? { spot: num(sc.spot), futures: num(sc.futures), filtered: !!sc.filtered }
        : null;
    return {
      strategies: num(d.strategies),
      runs: num(d.runs),
      runsActive: num(d.runs_active),
      keys: num(d.keys),
      sessions: num(d.sessions),
      scope: scope,
    };
  }


  /// 064: 告警数 = 被门禁拦住的策略条数。
  ///
  /// **空数组 = 0**(现在没有要处理的事), 不是"读不到" —— 这一格的正常态就是 0,
  /// 所以它不像其它格那样有"—"态。数据不在时(整个 overview 失败)才走不到这里。
  function alertCount() {
    const d = st.data && st.data.ok ? st.data.value : null;
    const list = (d && d.alerts) || [];
    return list.length;
  }

  /// 064: 告警格下面那几行明细(哪个策略被拦 + 复制的谁)。
  ///
  /// 只列前 3 个(与后端 take(3) 对齐); 更多请在策略库看 —— 概览要的是"有事, 点进去",
  /// 不是当告警控制台用。
  function renderAlertDetails() {
    const d = st.data && st.data.ok ? st.data.value : null;
    const list = (d && d.alerts) || [];
    // 0 条时说一句"没有要处理的" —— 概览上只看到一个光秃秃的 0, 用户不知道
    // "这是好事还是没查到"。**但不说"读不到"**: 走到这里说明 overview 整体是成功的,
    // alerts 恒是一个数组(空 = 真没事), 没有"读不到"这一态。
    if (!list.length) {
      return h("div", "dash-alert-none", t("dashAlertNone"));
    }
    const box = h("div", "dash-alert-list");
    for (const a of list) {
      const row = h("button", "dash-alert-row");
      row.type = "button";
      const txt = h("span", "dash-alert-row-txt");
      txt.appendChild(h("span", "dash-alert-row-id", a.id));
      // 有"复制的谁"才显示那半句 —— 没有就只说被拦, 不补一句猜的话。
      if (a.duplicate_of) {
        txt.appendChild(h("span", "dash-alert-row-of", " ← " + a.duplicate_of));
      }
      row.appendChild(txt);
      row.appendChild(h("span", "dash-alert-row-why", t("dashAlertBlocked")));
      row.addEventListener("click", () => R.navigate("#strategies/" + encodeURIComponent(a.id)));
      box.appendChild(row);
    }
    return box;
  }

  /// 画统计卡。
  ///
  /// 062: **四项**(策略/实例/密钥/会话) —— 全是本地事实。
  /// 刻意只有四项: fmz 的概览是六格, 但它六格里有两格是消息/账户(ricow 无账号)。
  /// 剩下的位置与其填一个要联网 15 秒的数字, 不如留白 —— 空白是诚实的, 假数字不是。
  function renderStats() {
    const c = counts(st.data);
    const card = R.ui.card({ title: t("dashStats") });
    const grid = h("div", "dash-grid");
    // 064: **每格都带链接**(fmz 的六格每格都是可点的, 点进去直接到那一域)。
    // `go` = 点击后去哪; `goLabel` = 无障碍名(图标按钮必须能读出"去哪")。
    const items = [
      { icon: "strategies", value: c.strategies, label: t("dashStrategyCount"),
        go: "#strategies", goLabel: t("dashStatsStrategyGo") },
      { icon: "runs", value: c.runs, label: t("dashRunCount"),
        go: "#runs", goLabel: t("dashStatsRunGo") },
      { icon: "keys", value: c.keys, label: t("dashKeyCount"),
        go: "#keys", goLabel: t("dashStatsKeyGo") },
      { icon: "chat", value: c.sessions, label: t("dashSessionCount"),
        go: "#chat", goLabel: t("dashStatsSessionGo") },
      // 064: 告警格(fmz 六格之一)。**数字 0 也显示** —— "没有要处理的事"是有效信息,
      // 与其它格"读不到才显示—"不同: 这个格的正常态就是 0。
      { icon: "alert", value: alertCount(), label: t("dashAlert"),
        go: "#strategies", goLabel: t("dashAlertGo"), alert: true },
    ];
    for (const it of items) {
      const cell = R.ui.stat({ icon: it.icon, value: it.value, label: it.label });
      // 064: **整格包成 button** —— 不是给 div 设 type(那没有语义, 键盘也到不了)。
      // 包一层 button 后: 鼠标能点、Tab 能到、Enter/Space 能触发、读屏能念出 aria-label。
      const link = h("button", "dash-stat-link");
      link.type = "button";
      link.title = it.goLabel;
      link.setAttribute("aria-label", it.label + " · " + it.goLabel);
      link.appendChild(cell);
      link.addEventListener("click", () => R.navigate(it.go));
      grid.appendChild(link);
    }
    card.appendChild(grid);
    // 064: 告警明细: 有事列出来, 0 条说一句"没有要处理的"(都不说话最糟)。
    card.appendChild(renderAlertDetails());
    return card;
  }

  /// 064: 画「美股代币覆盖」卡 —— ricow 对 fmz 的**差异化卖点**。
  ///
  /// 为什么放左列第一张(比统计卡还靠前): 这是用户选 ricow 而不是 fmz 的理由之一
  /// (币安官方机器人只开放少数 stock), 藏在第三屏等于没有。
  ///
  /// 三条纪律:
  /// ① 数字**来自 `/api/overview` 的 scope 字段**(与 `/api/markets` 同源) —— 不写死;
  /// ② 取不到显示「—」并说明原因, **不显示 0**(0 = 真的一个都没有);
  /// ③ 入口只**带路**到市场视图, **不偷偷改用户的 `ricow.toml`** ——
  ///    放宽视野是配置变更, 要用户自己在市场视图里显式确认。
  function renderBstock() {
    const c = counts(st.data);
    const card = R.ui.card({ title: t("dashBstock") });
    const body = h("div", "dash-bstock");

    if (!c.scope) {
      body.appendChild(h("div", "dash-bstock-unknown", t("dashBstockUnknown")));
      card.appendChild(body);
      return card;
    }

    // 两个数字 + 口径徽章。
    //
    // 066: 某一侧**取不到**时显示「—」而不是 0 —— 降级时后端给的那一半是空列表,
    // 直接打 `.spot` / `.futures` 会显示成"0 个"(假信息: 用户会以为 ricow
    // 一个交易对都不支持, 而不是网络没通)。
    const grid = h("div", "dash-bstock-grid");
    for (const it of [
      { label: t("dashBstockSpot"), value: c.scope.spot, ok: c.scope.spot_ok },
      { label: t("dashBstockFutures"), value: c.scope.futures, ok: c.scope.futures_ok },
    ]) {
      const cell = h("div", "dash-bstock-cell");
      const num = h("div", "dash-bstock-num", it.ok === false ? "—" : String(it.value));
      if (it.ok === false) num.classList.add("is-unreadable");
      cell.appendChild(num);
      cell.appendChild(h("div", "dash-bstock-label", it.label));
      grid.appendChild(cell);
    }
    body.appendChild(grid);

    // 066: 一侧不可读时给一句说明(不说的话, 用户只看到「—」不知道该查什么)。
    if (c.scope.spot_ok === false || c.scope.futures_ok === false) {
      body.appendChild(h("div", "dash-bstock-partial", t("dashBstockPartial")));
    }

    // 口径徽章: 「股票类视野」还是「全部交易对」—— 两个数字不说明口径,
    // 同一份视野在 all=true 下数字会大一个量级, 不标口径用户会以为数字在跳。
    const badge = h("span", "dash-bstock-badge" + (c.scope.filtered ? " is-filtered" : ""));
    badge.textContent = c.scope.filtered ? t("dashBstockScopeStock") : t("dashBstockScopeAll");
    body.appendChild(badge);

    // 优势说明(一句话, 不写长文案 —— 概览要的是扫一眼就懂)。
    body.appendChild(h("div", "dash-bstock-edge", t("dashBstockEdge")));

    // 入口: 带去市场视图。**不带 query** —— markets.js 的 activate(param) 把 param
    // 当"交易对符号"用, 带 `?all=1` 会被当成一个不存在的交易对去查(404)。
    // 到市场视图后由用户点"全部"开关显式放宽, 那才是"用户自己决定的"。
    const go = R.ui.button({
      label: t("dashBstockSeeAll"),
      kind: "text",
      onClick: () => R.navigate("#markets"),
    });
    body.appendChild(go);

    card.appendChild(body);
    return card;
  }

  /// 画快捷入口六宫格。
  function renderQuick() {
    const card = R.ui.card({ title: t("dashQuick") });
    const grid = h("div", "dash-quick");
    const entries = [
      { label: t("dashQuickOpenStrategies"), icon: "strategies", hash: "#strategies" },
      { label: t("dashQuickNewStrategy"), icon: "plus", hash: "#strategies" },
      { label: t("dashQuickOpenMarkets"), icon: "markets", hash: "#markets" },
      { label: t("dashQuickOpenRuns"), icon: "runs", hash: "#runs" },
      { label: t("dashQuickOpenKeys"), icon: "keys", hash: "#keys" },
      { label: t("dashQuickNewChat"), icon: "chat", hash: "#chat" },
      // 064: AI 助手入口(fmz 六格里有"AI 助手"与"分析工具"; 分析工具 ricow 无对应物,
      // 不留占位 —— AI 助手是真实存在的: 对话视图本身就是)。
      { label: t("dashQuickAiChat"), icon: "chat", hash: "#chat" },
    ];
    for (const e of entries) {
      grid.appendChild(
        R.ui.quick({
          label: e.label,
          icon: e.icon,
          onClick: () => R.navigate(e.hash),
        })
      );
    }
    card.appendChild(grid);
    return card;
  }

  /// 画「运行状态」卡(替代 fmz 的账户卡 —— ricow 无账号)。
  function renderRunning() {
    const c = counts(st.data);
    const card = R.ui.card({ title: t("dashRunning") });
    const body = h("div", "dash-running");
    const row = h("div", "dash-running-row");
    const left = h("div", "dash-running-cell");
    left.appendChild(h("div", "dash-running-num", c.runs === null ? "—" : String(c.runs)));
    left.appendChild(h("div", "dash-running-label", t("dashRunningCount")));
    const right = h("div", "dash-running-cell");
    right.appendChild(h("div", "dash-running-num", c.runsActive === null ? "—" : String(c.runsActive)));
    right.appendChild(h("div", "dash-running-label", t("dashTotal")));
    row.appendChild(left);
    row.appendChild(right);
    body.appendChild(row);

    // 实例清单: 聚合端点只给计数, 不给明细(概览要的是"一眼看到现状")。
    // 故这里只给一句状态说明 + 一个"去运行视图看"的入口, 不摆假清单。
    const note = h("div", "dash-running-empty", c.runs ? t("dashHasRun") : t("dashNoRun"));
    body.appendChild(note);
    const go = R.ui.button({
      label: t("dashQuickOpenRuns"),
      kind: "text",
      onClick: () => R.navigate("#runs"),
    });
    body.appendChild(go);
    card.appendChild(body);
    return card;
  }

  /// 064: 画「最近操作」卡 —— fmz 的 dashboard 有同名的卡位, 我们填**真实数据**。
  ///
  /// 数据源是 `/api/overview` 的 `recent_sessions`(按 `updated_at` 倒序前 3 个会话)。
  /// 为什么用会话: ricow 本地**真正持久化**的"人最近干了什么"只有会话
  /// (`web_sessions.updated_at`)。回测作业只留 5 分钟且不落库 —— 拿它当"最近操作",
  /// 用户 5 分钟后回来看就是一张空卡, 那是个会骗人的卡。
  ///
  /// 也**必须走聚合端点**: 概览页常驻 SSE + 每 5 秒三路轮询, 浏览器同源连接上限只有 6,
  /// 前端再并发一个 `/api/sessions` 就会排队(062 实测的坑)。
  function renderRecent() {
    const card = R.ui.card({ title: t("dashRecent") });
    const body = h("div", "dash-recent");
    const c = counts(st.data);
    // recent_sessions 只在数据成功取到时才有; 取不到时不给清单(与"—"同口径)。
    const list = (st.data && st.data.ok && st.data.value && st.data.value.recent_sessions) || [];

    if (!list.length) {
      body.appendChild(h("div", "dash-recent-empty", c.sessions === null ? t("dashBstockUnknown") : t("dashRecentEmpty")));
      const go = R.ui.button({
        label: t("dashRecentOpen"),
        kind: "text",
        onClick: () => R.navigate("#chat"),
      });
      body.appendChild(go);
      card.appendChild(body);
      return card;
    }

    const ul = h("ul", "dash-recent-list");
    for (const s of list) {
      const li = h("li", "dash-recent-item");
      const btn = h("button", "dash-recent-link");
      btn.type = "button";
      // 标题为空(首条消息还没到) → 明说"未命名", 不拿 id 冒充标题。
      const title = s.title && s.title.trim() ? s.title : t("dashRecentUntitled");
      btn.appendChild(h("span", "dash-recent-title", title));
      // 相对时间: 复用策略库里那套(同一口径, 不另写一份)。
      const rel = window.Ricow && window.Ricow.sg && window.Ricow.sg.fmtRelTime
        ? window.Ricow.sg.fmtRelTime(s.updated_at)
        : null;
      btn.appendChild(h("span", "dash-recent-time", rel || ""));
      btn.addEventListener("click", () => R.navigate("#chat/" + encodeURIComponent(s.id)));
      li.appendChild(btn);
      ul.appendChild(li);
    }
    body.appendChild(ul);
    // 一句口径说明: 告诉用户"为什么最近操作只有会话" —— 免得以为回测记录丢了。
    body.appendChild(h("div", "dash-recent-hint", t("dashRecentHint")));
    card.appendChild(body);
    return card;
  }

  /// 画「开发文档」卡(fmz 的文档卡位置, 内容换成 ricow 自己的帮助入口)。
  function renderDocs() {
    const card = R.ui.card({ title: t("dashDocs") });
    const grid = h("div", "dash-docs");
    const entries = [
      { title: t("dashDocsGlossary"), desc: t("dashDocsGlossaryDesc") },
      { title: t("dashDocsUserGuide"), desc: t("dashDocsUserGuideDesc") },
      { title: t("dashDocsLua"), desc: t("dashDocsLuaDesc") },
      { title: t("dashDocsSafety"), desc: t("dashDocsSafetyDesc") },
    ];
    for (const e of entries) {
      const link = h("button", "ui-doc-link");
      link.type = "button";
      const txt = h("div", "ui-doc-text");
      txt.appendChild(h("div", "ui-doc-title", e.title));
      txt.appendChild(h("div", "ui-doc-desc", e.desc));
      link.appendChild(txt);
      const arrow = h("span", "ui-doc-arrow");
      arrow.appendChild(R.ui.icon("arrow", 14));
      link.appendChild(arrow);
      // 文档卡 → 打开帮助抽屉(帮助面板按当前视图组织内容, 概览落在"术语"段)。
      link.addEventListener("click", () => {
        const btn = document.getElementById("help-btn");
        if (btn) btn.click();
      });
      grid.appendChild(link);
    }
    card.appendChild(grid);
    return card;
  }

  /// 整页渲染。
  function render() {
    const root = st.root;
    if (!root) return;
    root.innerHTML = "";

    // 062: **面包屑不在这画** —— 由 router.js 的 `renderCrumbs` 统一写在 `#crumbs` 里
    // (fmz 的面包屑是全局的, 各视图各画一份必然格式不一)。

    if (st.error) {
      const card = R.ui.card({ title: t("dashLoadFailed") });
      const body = h("div", "dash-error");
      body.appendChild(h("div", "dash-error-msg", st.error));
      body.appendChild(
        R.ui.button({
          label: t("dashRetry"),
          kind: "default",
          onClick: () => {
            loadAll();
          },
        })
      );
      card.appendChild(body);
      root.appendChild(card);
      return;
    }

    const grid = h("div", "dash-layout");
    const left = h("div", "dash-col");
    // 064: bStock 覆盖卡放**最前面** —— 它是 ricow 的差异化卖点, 不该排在统计卡后面。
    left.appendChild(renderBstock());
    left.appendChild(renderStats());
    // 064: 最近操作卡(fmz 有同名卡位)。
    left.appendChild(renderRecent());
    left.appendChild(renderQuick());
    left.appendChild(renderDocs());
    const right = h("div", "dash-col");
    right.appendChild(renderRunning());
    grid.appendChild(left);
    grid.appendChild(right);
    root.appendChild(grid);
  }

  R.views.dashboard = {
    activate: function () {
      const el = document.getElementById("view-dashboard");
      if (!el) return;
      st.root = el;
      render();
      // 每次进入都重取 —— 概览的价值就是"现在的数字", 缓存过的数字是负资产。
      loadAll();
    },
    deactivate: function () {
      st.root = null;
    },
  };
})();
