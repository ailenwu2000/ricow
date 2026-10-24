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
    if (!v || !v.ok || !v.value) {
      return { strategies: null, runs: null, runsActive: null, keys: null, sessions: null };
    }
    const d = v.value;
    const num = (x) => (typeof x === "number" && Number.isFinite(x) ? x : null);
    return {
      strategies: num(d.strategies),
      runs: num(d.runs),
      runsActive: num(d.runs_active),
      keys: num(d.keys),
      sessions: num(d.sessions),
    };
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
    const items = [
      { icon: "strategies", value: c.strategies, label: t("dashStrategyCount") },
      { icon: "runs", value: c.runs, label: t("dashRunCount") },
      { icon: "keys", value: c.keys, label: t("dashKeyCount") },
      { icon: "chat", value: c.sessions, label: t("dashSessionCount") },
    ];
    for (const it of items) {
      grid.appendChild(R.ui.stat({ icon: it.icon, value: it.value, label: it.label }));
    }
    card.appendChild(grid);
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
    left.appendChild(renderStats());
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
