// Ricow Web 前端「运行」视图 (032 US4, FR-022 ~ FR-028):
//   实例一览(5s 轮询, 页面不可见暂停) / 启动对话框(dry_run / demo / live 三模式,
//   live 两步门禁: 逐字短语 → 首次风险披露确认) / 停止确认(可选同时撤单平仓) /
//   行内日志(SSE 流, 只读)。
// 注册为 `R.views.runs`; 数据全部走 R.api 的 /api/runs 与 /api/strategies/{id}/start|stop、
// /api/risk-ack; 日志走 /api/logs/{name}/stream(token 在查询串, 与对话视图各一条独立流)。
//
// 过期响应: 视图级"代际令牌" gen —— activate/deactivate 自增, 在途响应回来比对捕获值,
// 不一致即丢弃。安全: 不写浏览器持久存储; 服务端数据一律 textContent / .value(不拼 innerHTML)。
"use strict";

(function () {
  const R = window.Ricow;
  const t = (key) => R.t(key);

  // ---------- 双语文案(并入公共字典; 静态标签走 data-zh/data-en) ----------
  Object.assign(R.TEXT.zh, {
    rnTitle: "实盘",
    rnNewLive: "新建实盘",
    rnPollHint: "每 5 秒自动刷新;页面切到后台时暂停。",
    rnNoInstances: "暂无策略实例(到策略页保存实例配置后会出现在这里)。",
    rnNoStrategies: "还没有任何策略,先到策略页创建。",
    rnGoStrategies: "去策略页",
    rnLoadFailed: "加载失败: ",
    rnRetry: "重试",
    rnLoading: "加载中…",
    rnThName: "名称",
    rnThMode: "模式",
    rnThStatus: "状态",
    rnThPair: "标的",
    rnThMarket: "市场",
    rnMarketAll: "全部",
    rnMarketSpot: "现货",
    rnMarketFutures: "合约",
    rnThNet: "盈利",
    rnThTrades: "成交数",
    rnThUptime: "运行时长",
    rnThSource: "来源",
    rnThAction: "操作",
    rnStart: "启动",
    rnStop: "停止",
    rnStatusRunning: "运行中",
    rnStatusStopped: "已停止",
    rnStatusUnknown: "状态不可知",
    rnModeUnknown: "未知",
    rnSrcDaemonTip: "守护进程在线,状态以它为准。",
    rnSrcLedgerTip: "守护进程不在线,显示台账最近一次记录,当前状态不可知。",
    rnSrcNoneTip: "仅有实例配置,从未运行或台账无记录。",
    rnStartTitle: "新建实盘",
    rnStartStrategy: "运行策略",
    // 063: fmz 的「新建实盘」字段(fmz 的 /m/add-robot)。
    rnLiveName: "实盘名称",
    rnLivePair: "交易对",
    rnLivePairOutOfScope:
      "策略自带的交易对 {0} 不在当前交易对视野里, 已换成 {1}(要跑原交易对请在设置里把交易对视野改为全部)。若策略有价格类参数, 请按新交易对的价格重新确认。",
    rnLiveParams: "策略参数",
    rnLiveReset: "重置参数",
    rnResetDone: "已把 {n} 个参数恢复成清单默认值。",
    rnResetNothing: "没有可恢复的参数 —— 这个策略的参数都没写默认值(必填项就是这样), 改动需要你自己调整。",
    rnLiveParamsLoadFail: "读不到该策略的参数清单:",
    rnLiveParamsUnavailable: "参数表单不可用(前端模块未加载)。",
    rnLiveNoParams: "该策略没有声明参数 —— 只用默认值运行。",
    rnLiveBuiltinNote: "内置策略的代码与参数随版本发布, 不能直接改; 请先在策略页「复制」成你自己的策略。",
    rnLiveBuiltinBlock: "内置策略不能直接改参数 —— 请先在策略页「复制」成你自己的策略, 再新建实盘。",
    rnStratSearchPh: "输入策略名搜索",
    rnStratNoHit: "没有匹配的策略",
    rnStartMode: "模式",
    rnModeDryRun: "Dry Run(演练,不下单)",
    rnModeDemo: "Demo(币安测试网)",
    rnModeLive: "Live(实盘,真实下单)",
    rnPhraseHint: "请逐字输入确认短语: ",
    rnPhraseEmpty: "确认短语不能为空",
    rnDemoKeysMissing: "请先在密钥页添加一套测试网(demo)凭据。",
    rnGoSettings: "去密钥页",
    rnAckTitle: "首次启动 Live:请先阅读风险披露",
    rnAckPhraseHint: "输入「确认风险」以确认已阅读: ",
    rnAckGo: "确认风险",
    rnAckOk: "风险确认已记录,请继续输入实盘确认短语。",
    rnStartGo: "创建并启动",
    rnStarting: "启动中…",
    rnStartOk: "已启动: ",
    rnStopTitle: "停止策略",
    rnStopConfirm: "确认停止",
    rnStopCloseAll: "同时撤销全部挂单并平仓",
    rnStopGo: "停止",
    rnStopping: "停止中…",
    rnStopOk: "已停止: ",
    // 058: kill switch。短语本身**不进字典** —— 它必须与后端逐字比对, 写成可替换文案
    // 就等于给了它一个被翻译掉的入口(后端只认中文字面量)。这里只放"提示"不含短语值。
    rnKillAll: "停止全部",
    rnKillTitle: "停止全部运行中的策略",
    rnKillWarn: "将停止当前所有运行中的实例。",
    rnKillKeep: "后台守护进程",
    rnKillKeepDaemon: "daemon 保持运行(停完策略后还要看现场)。",
    rnKillCloseAll: "同时撤销全部挂单并平仓(不可逆)",
    rnKillGo: "停止全部",
    rnKillOk: "已停止 ",
    rnKillNone: "当前没有运行中的实例(无需停止)。",
    rnLogOf: "日志: ",
    rnLogClose: "收起",
    rnLogToggle: "点击展开/收起日志",
  });
  Object.assign(R.TEXT.en, {
    rnTitle: "Live",
    rnNewLive: "New live trading",
    rnPollHint: "Auto-refreshes every 5s; paused while the page is hidden.",
    rnNoInstances: "No strategy instances yet (save one in Strategies and it shows up here).",
    rnNoStrategies: "No strategies yet — create one in Strategies first.",
    rnGoStrategies: "Go to Strategies",
    rnLoadFailed: "Load failed: ",
    rnRetry: "Retry",
    rnLoading: "Loading…",
    rnThName: "Name",
    rnThMode: "Mode",
    rnThStatus: "Status",
    rnThPair: "Pair",
    rnThMarket: "Market",
    rnMarketAll: "All",
    rnMarketSpot: "Spot",
    rnMarketFutures: "Perps",
    rnThNet: "Profit",
    rnThTrades: "Fills",
    rnThUptime: "Uptime",
    rnThSource: "Source",
    rnThAction: "Actions",
    rnStart: "Start",
    rnStop: "Stop",
    rnStatusRunning: "running",
    rnStatusStopped: "stopped",
    rnStatusUnknown: "state unknown",
    rnModeUnknown: "unknown",
    rnSrcDaemonTip: "daemon is online; its view is authoritative.",
    rnSrcLedgerTip: "daemon is offline; the ledger's last record is shown — the current state is unknown.",
    rnSrcNoneTip: "Only an instance config exists — never run, or no ledger record.",
    rnStartTitle: "New live trading",
    rnStartStrategy: "Strategy to run",
    // 063: fmz's "new live trading" fields (fmz /m/add-robot).
    rnLiveName: "Live name",
    rnLivePair: "Trading pair",
    rnLivePairOutOfScope:
      "This strategy's own pair {0} is outside the current pair scope, so {1} was used instead (to trade the original pair, widen the pair scope to all in Settings). If the strategy has price parameters, re-check them against the new pair's price.",
    rnLiveParams: "Strategy parameters",
    rnLiveReset: "Reset parameters",
    rnResetDone: "Restored {n} parameters to their manifest defaults.",
    rnResetNothing: "Nothing to restore — none of this strategy's parameters declare a default (required ones don't), so the change is yours to make.",
    rnLiveParamsLoadFail: "Could not read this strategy's parameter manifest:",
    rnLiveParamsUnavailable: "Parameter form unavailable (frontend module not loaded).",
    rnLiveNoParams: "This strategy declares no parameters — it runs on defaults.",
    rnLiveBuiltinNote: "Built-in strategy code and parameters ship with the release and cannot be edited here; copy it to your own strategy first.",
    rnLiveBuiltinBlock: "Built-in strategies cannot have their parameters changed — copy it to your own strategy in the Strategies view first.",
    rnStratSearchPh: "Search strategies by name",
    rnStratNoHit: "No matching strategy",
    rnStartMode: "Mode",
    rnModeDryRun: "Dry Run (no real orders)",
    rnModeDemo: "Demo (Binance testnet)",
    rnModeLive: "Live (real orders)",
    rnPhraseHint: "Type the confirmation phrase exactly: ",
    rnPhraseEmpty: "The confirmation phrase must not be empty",
    rnDemoKeysMissing: "Add a testnet (demo) credential set on the Keys page first.",
    rnGoSettings: "Go to Keys",
    rnAckTitle: "First live start: read the risk disclosure",
    rnAckPhraseHint: "Type 确认风险 to confirm you have read it: ",
    rnAckGo: "Acknowledge",
    rnAckOk: "Risk acknowledgement recorded. Now enter the live confirmation phrase.",
    rnStartGo: "Create and start",
    rnStarting: "Starting…",
    rnStartOk: "Started: ",
    rnStopTitle: "Stop strategy",
    rnStopConfirm: "Confirm stop",
    rnStopCloseAll: "Also cancel all open orders and close positions",
    rnStopGo: "Stop",
    rnStopping: "Stopping…",
    rnStopOk: "Stopped: ",
    rnKillAll: "Stop all",
    rnKillTitle: "Stop all running strategies",
    rnKillWarn: "This stops every instance that is currently running.",
    rnKillKeep: "The daemon keeps running",
    rnKillKeepDaemon: "daemon keeps running (you still need to look at the scene afterwards).",
    rnKillCloseAll: "Also cancel all open orders and close positions (irreversible)",
    rnKillGo: "Stop all",
    rnKillOk: "Stopped ",
    rnKillNone: "No running instances (nothing to stop).",
    rnLogOf: "Log: ",
    rnLogClose: "Collapse",
    rnLogToggle: "Click to expand/collapse log",
  });

  // ---------- 常量与状态 ----------
  const POLL_MS = 5000; // 实例表轮询周期(FR-022)。
  const LOG_TAIL_LINES = 200; // 打开日志先给最近多少行(服务端还会再夹一次硬上限)。
  const LOG_DOM_MAX = 500; // 日志 DOM 只留最近这么多行, 长时间开着也不越堆越慢。
  const NUM_COLS = 9; // 列数: 名称/模式/状态/标的/净收益/成交数/运行时长/来源/操作。
  // 三种启动模式 → 各自的双语标签键(值本身 dry_run/demo/live 是技术词, 中英一致)。
  const MODE_KEYS = { dry_run: "rnModeDryRun", demo: "rnModeDemo", live: "rnModeLive" };

  let mounted = false;
  // 代际令牌: 每次激活/离开自增, 过期响应一律不碰 DOM。
  let gen = 0;
  let active = false; // 视图当前是否激活(轮询暂停的依据之一)。
  let listData = null; // 最近一次 /api/runs 响应(切语言直接重画, 不重新取数)。
  let loadFailed = false; // 最近一次取数是否失败(切语言时决定重画错误框还是表体)。
  let lastLoadErr = null;
  let listMsgIsErr = false; // 列表消息当前是否为错误(轮询成功只清错误, 保住成功提示)。
  let pollTimer = 0;
  // 行内日志面板: 宿主元素持久(轮询重画表体后原样插回, 流不断开)。
  let expandedName = null;
  let logStream = null;
  let logBox = null;
  let logTitle = null;
  let logLines = null;
  let logTr = null;

  const $ = (id) => document.getElementById(id);

  /// 建元素: 文本一律 textContent(服务端数据即使含 <> 也只作文本)。
  function h(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  /// 静态双语标签属性(仅用于本文件内无插值的固定文案; 服务端文本绝不走这里)。
  function bi(key) {
    return ' data-zh="' + R.TEXT.zh[key] + '" data-en="' + R.TEXT.en[key] + '"';
  }

  /// 行内消息: text 空串清空; kind: "" | "ok" | "err"。
  function setMsg(el, text, kind) {
    el.textContent = text || "";
    el.className = "sg-msg" + (kind ? " " + kind : "");
  }

  function setListMsg(text, kind) {
    listMsgIsErr = kind === "err";
    setMsg($("rn-list-msg"), text, kind);
  }

  // ---------- 一次性挂载(骨架只建一次; 数据每次激活重取) ----------

  function mount() {
    if (mounted) return;
    const host = $("view-runs");
    host.innerHTML =
      '<div class="sg-wrap">' +
      '<div class="rn-listview">' +
      '<div class="sg-head"><h2 class="sg-title"' + bi("rnTitle") + ">" + R.TEXT.zh.rnTitle + "</h2>" +
      '<span id="rn-poll-hint" class="rn-poll-hint"' + bi("rnPollHint") + ">" +
      R.TEXT.zh.rnPollHint + "</span>" +
      // 063: 「新建实盘」入口(fmz 的 /m/robots 表头就有这个按钮)。
      // 此前只有**每行**的「启动」按钮 —— 没有实例时用户无处可点, 得先有个策略才能启动,
      // 顺序反了(fmz 的顺序是"先建实盘, 再看它跑起来")。
      '<button id="rn-new-live" class="sg-btn sg-btn-primary"' + bi("rnNewLive") + ">" +
      R.TEXT.zh.rnNewLive + "</button>" +
      // 058: 「停止全部」挂在运行视图表头, 不进顶栏 —— 它是运行域的动作, 不是全局导航。
      '<button id="rn-kill-all" class="sg-btn sg-btn-danger rn-kill-all"' + bi("rnKillAll") + ">" +
      R.TEXT.zh.rnKillAll + "</button></div>" +
      '<div id="rn-list-msg" class="sg-msg" aria-live="polite"></div>' +
      '<div class="rn-table-scroll"><table class="rn-table"><thead><tr>' +
      "<th" + bi("rnThName") + ">" + R.TEXT.zh.rnThName + "</th>" +
      "<th" + bi("rnThMode") + ">" + R.TEXT.zh.rnThMode + "</th>" +
      "<th" + bi("rnThStatus") + ">" + R.TEXT.zh.rnThStatus + "</th>" +
      "<th" + bi("rnThPair") + ">" + R.TEXT.zh.rnThPair + "</th>" +
      "<th" + bi("rnThNet") + ">" + R.TEXT.zh.rnThNet + "</th>" +
      "<th" + bi("rnThTrades") + ">" + R.TEXT.zh.rnThTrades + "</th>" +
      "<th" + bi("rnThUptime") + ">" + R.TEXT.zh.rnThUptime + "</th>" +
      "<th" + bi("rnThSource") + ">" + R.TEXT.zh.rnThSource + "</th>" +
      // 064: 「市场」列 —— **列头本身就是筛选**(fmz 的"全部种类"形态)。
      // 放在"标的"后: 市场是标的的限定语, 读序自然; 也不与"操作"挤在一起误点。
      //
      // **`data-zh` 必须挂内层 `<span>`, 不能挂 `<th>`**: `applyLang` 换语言用的是
      // `node.textContent = ...`, 而 textContent 赋值会**清空全部子节点** ——
      // 挂在 `<th>` 上的话, 切一次语言这个 `<select>` 就被抹掉(2026-10-25 实测:
      // 首屏就是"已应用过一次语言"的状态, 于是 select 从头到尾不存在)。
      // 这与侧栏导航项 `data-zh` 要挂 `.nav-text` 是同一个坑(见项目记忆)。
      '<th class="rn-th-market"><span class="rn-th-market-label"' + bi("rnThMarket") + ">" +
      R.TEXT.zh.rnThMarket + "</span>" +
      '<select id="rn-market-filter" class="rn-market-filter" ' +
      'aria-label="' + R.TEXT.zh.rnThMarket + '">' +
      "<option value=\"all\">" + R.TEXT.zh.rnMarketAll + "</option>" +
      "<option value=\"spot\">" + R.TEXT.zh.rnMarketSpot + "</option>" +
      "<option value=\"futures\">" + R.TEXT.zh.rnMarketFutures + "</option>" +
      "</select></th>" +
      "<th" + bi("rnThAction") + ">" + R.TEXT.zh.rnThAction + "</th>" +
      "</tr></thead>" +
      '<tbody id="rn-tbody"></tbody></table></div>' +
      "</div>" +
      '</div>';

    // 回前台立即补一次刷新(轮询在不可见期间是暂停的)。
    document.addEventListener("visibilitychange", onVisibility);
    // 058: 停止全部入口。绑一次即可 —— 按钮是 mount() 里静态建的, 不参与列表重绘。
    // openKillModal 本身是同步的(只开弹窗, 提交在弹窗内), 错误由弹窗自己的 errLine 承载,
    // 所以这里**不能** .catch() —— 那会把同步函数的 undefined 当 Promise 而在运行期抛
    // TypeError(真机走查 058 抓到过; 静态门看不见这一类)。
    $("rn-kill-all").addEventListener("click", () => openKillModal());
    // 063: 「新建实盘」入口(fmz 的 /m/robots 表头就有)。
    // openStartModal 是 **async**(要先取策略清单), 所以这里要 .catch() ——
    // 与上面的 openKillModal 不同(它是同步函数, **不能** .catch(), 见上方注释)。
    $("rn-new-live").addEventListener("click", () =>
      openStartModal().catch((e) => setListMsg((e && e.message) || String(e), "err"))
    );
    // 064: 市场筛选(列头那个 select)。**只做前端过滤** ——
    // 实例数本来就不多(单机程序), 为筛选加一个后端参数不值得; 而且前端过滤
    // 能立刻响应, 不用等一轮轮询。
    const mkFilter = $("rn-market-filter");
    if (mkFilter) {
      mkFilter.addEventListener("change", () => {
        const want = mkFilter.value;
        const tbody = $("rn-tbody");
        if (!tbody) return;
        for (const tr of tbody.querySelectorAll("tr.rn-row")) {
          const badge = tr.querySelector("[data-market]");
          const mk = badge ? badge.dataset.market : "";
          tr.hidden = want !== "all" && mk !== want;
        }
      });
    }
    mounted = true;
  }

  function onVisibility() {
    if (!document.hidden && active) refreshQuiet(gen);
  }

  // ---------- 取数与渲染 ----------

  function startPolling() {
    if (pollTimer) clearInterval(pollTimer);
    pollTimer = setInterval(() => {
      // 页面不可见 / 视图未激活: 暂停本轮取数, 回前台由 visibilitychange 立即补一次。
      if (document.hidden || !active) return;
      refreshQuiet(gen);
    }, POLL_MS);
  }

  /// 静默刷新: 已有表体的前提下每 5s 重取; 成功后只清错误提示(保住「已启动/已停止」)。
  async function refreshQuiet(g) {
    let data;
    try {
      data = await R.api("/api/runs");
    } catch (err) {
      if (g !== gen) return;
      setListMsg(t("rnLoadFailed") + ((err && err.message) || String(err)), "err");
      return;
    }
    if (g !== gen) return;
    loadFailed = false;
    lastLoadErr = null;
    listData = data;
    renderRows();
    if (listMsgIsErr) setListMsg("", "");
  }

  /// 激活后的首次取数: 先给加载行, 失败整表换错误框 + 重试。
  async function enter(g) {
    loadFailed = false;
    lastLoadErr = null;
    const tbody = $("rn-tbody");
    tbody.innerHTML = "";
    const tr = document.createElement("tr");
    const td = h("td", "sg-loading", t("rnLoading"));
    td.colSpan = NUM_COLS;
    tr.appendChild(td);
    tbody.appendChild(tr);
    setListMsg("", "");
    let data;
    try {
      data = await R.api("/api/runs");
    } catch (err) {
      if (g !== gen) return;
      loadFailed = true;
      lastLoadErr = err;
      renderLoadError(err, () => enter(gen));
      return;
    }
    if (g !== gen) return;
    listData = data;
    renderRows();
  }

  function renderLoadError(err, retry) {
    const tbody = $("rn-tbody");
    tbody.innerHTML = "";
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = NUM_COLS;
    const box = h("div", "sg-error-box");
    box.appendChild(h("div", "sg-msg err", t("rnLoadFailed") + ((err && err.message) || String(err))));
    const btn = h("button", "sg-btn", t("rnRetry"));
    btn.type = "button";
    btn.addEventListener("click", retry);
    box.appendChild(btn);
    td.appendChild(box);
    tr.appendChild(td);
    tbody.appendChild(tr);
  }

  function renderRows() {
    const tbody = $("rn-tbody");
    tbody.innerHTML = "";
    // 059 UX-06/13: 「停止全部」是**不可逆的危险动作**, 没有任何实例时它无事可停 ——
    // 此前它一直亮在空表格上方(旁边就是"暂无策略实例"), 视觉上把最危险的东西摆在
    // 一个什么都没有的页面上。有实例才出现。
    const killAll = $("rn-kill-all");
    if (killAll) killAll.hidden = !listData || listData.length === 0;
    if (!listData || !listData.length) {
      const tr = document.createElement("tr");
      tr.className = "rn-empty";
      // B4: 空表不该只有一句"暂无实例" —— 给一个去策略页的入口(实例是在那里配的)。
      const td = h("td", null);
      td.colSpan = NUM_COLS;
      td.appendChild(
        R.emptyState(t("rnNoInstances"), { label: t("rnGoStrategies"), view: "strategies" })
      );
      tr.appendChild(td);
      tbody.appendChild(tr);
      return;
    }
    for (const item of listData) tbody.appendChild(buildRow(item));
    insertLogBox();
  }

  function buildRow(item) {
    const tr = h("tr", "rn-row");
    tr.dataset.name = item.name;

    // 名称: 点开/收起行内日志。
    const nameTd = h("td");
    const nameBtn = h("button", "rn-name-btn", item.name);
    nameBtn.type = "button";
    nameBtn.title = t("rnLogToggle");
    nameBtn.addEventListener("click", () => toggleLog(item.name));
    nameTd.appendChild(nameBtn);
    tr.appendChild(nameTd);

    // 模式徽标(缺失/未知 → unknown, 不猜)。
    const modeTd = h("td");
    const knownMode = Object.prototype.hasOwnProperty.call(MODE_KEYS, item.mode);
    modeTd.appendChild(
      h("span", "sg-badge rn-mode-" + (knownMode ? item.mode : "unknown"), knownMode ? item.mode : t("rnModeUnknown"))
    );
    tr.appendChild(modeTd);

    // 状态: 绿点=运行中; 灰点=已停止/状态不可知(只有 daemon 在线才敢下"已停止"的结论)。
    const statusTd = h("td", "rn-status");
    statusTd.appendChild(h("span", "rn-dot" + (item.running ? " on" : "")));
    statusTd.appendChild(h("span", null, statusLabel(item)));
    tr.appendChild(statusTd);

    // 064: 市场徽章(现货/合约)。**读不到就不画徽章, 显示 "--"** ——
    // 猜一个市场比不显示更糟: 用户会按猜的那个去配交易所参数。
    const marketTd = h("td");
    const mk = (item.market || "").trim().toLowerCase();
    if (mk === "spot" || mk === "futures") {
      const badge = h("span", "sg-badge rn-market-" + mk, mk === "spot" ? t("rnMarketSpot") : t("rnMarketFutures"));
      badge.dataset.market = mk;
      marketTd.appendChild(badge);
    } else {
      marketTd.appendChild(h("span", "rn-muted", "--"));
    }
    tr.appendChild(marketTd);

    // 标的 / 净收益 / 成交数: 缺失一律 "--", 不伪造(PnL 走服务端字符串, 避免精度失真)。
    tr.appendChild(h("td", null, item.pair || "--"));
    tr.appendChild(h("td", "rn-num", item.pnl ? String(item.pnl.net) : "--"));
    tr.appendChild(h("td", "rn-num", item.pnl ? String(item.pnl.trade_count) : "--"));

    // 运行时长。
    tr.appendChild(h("td", "rn-num", fmtUptime(item.uptime_secs)));

    // 来源徽标(title 悬停解释三态口径)。
    const srcTd = h("td");
    const srcBadge = h("span", "sg-badge rn-src-" + item.source, item.source);
    srcBadge.title = srcTip(item.source);
    srcTd.appendChild(srcBadge);
    tr.appendChild(srcTd);

    // 操作: 按状态二选一(运行中 → 停止, 否则 → 启动)。
    const opTd = h("td");
    const opBtn = h(
      "button",
      "sg-btn sg-btn-mini" + (item.running ? "" : " sg-btn-primary"),
      item.running ? t("rnStop") : t("rnStart")
    );
    opBtn.type = "button";
    opBtn.addEventListener("click", () => {
      if (item.running) openStopModal(item.name);
      else openStartModal(item.name);
    });
    opTd.appendChild(opBtn);
    tr.appendChild(opTd);
    return tr;
  }

  /// 状态文案三态: 运行中 / 已停止(仅 daemon 在线可下此结论) / 状态不可知。
  function statusLabel(item) {
    if (item.running) return t("rnStatusRunning");
    if (item.source === "daemon") return t("rnStatusStopped");
    return t("rnStatusUnknown");
  }

  function srcTip(source) {
    if (source === "daemon") return t("rnSrcDaemonTip");
    if (source === "ledger") return t("rnSrcLedgerTip");
    return t("rnSrcNoneTip");
  }

  /// 秒数 → 紧凑时长; null → "--"。C11: 实现统一到 `R.fmt.dur`, 调用点不变。
  function fmtUptime(secs) {
    return R.fmt.dur(secs);
  }

  // ---------- 行内日志面板(SSE, 只读; 与对话视图各一条独立流) ----------

  function ensureLogBox() {
    if (logBox) return;
    logBox = h("div", "rn-log-box");
    const head = h("div", "rn-log-head");
    logTitle = h("span", "rn-log-title");
    head.appendChild(logTitle);
    const closeBtn = h("button", "rn-log-close", "×");
    closeBtn.type = "button";
    closeBtn.title = t("rnLogClose");
    closeBtn.addEventListener("click", () => {
      collapseLog();
    });
    head.appendChild(closeBtn);
    logBox.appendChild(head);
    logLines = h("div", "rn-log-lines");
    logBox.appendChild(logLines);
  }

  function toggleLog(name) {
    if (expandedName === name) {
      collapseLog();
      return;
    }
    closeStream();
    expandedName = name;
    ensureLogBox();
    logLines.textContent = "";
    logTitle.textContent = t("rnLogOf") + " " + name;
    openStream(name);
    renderRows(); // 重建表体并把面板插到对应行之后(面板宿主本身不重建, 流不断)。
  }

  function collapseLog() {
    closeStream();
    expandedName = null;
    if (logTr && logTr.parentNode) logTr.parentNode.removeChild(logTr);
    logTr = null;
    if (logLines) logLines.textContent = "";
  }

  function openStream(name) {
    const url =
      "/api/logs/" +
      encodeURIComponent(name) +
      "/stream?lines=" +
      LOG_TAIL_LINES +
      "&token=" +
      encodeURIComponent(R.TOKEN);
    const handle = R.sse(url, {
      onmessage: (ev) => {
        if (logStream !== handle) return; // 切行/收起后旧流可能还有余帧, 丢
        let event;
        try {
          event = JSON.parse(ev.data);
        } catch (_) {
          return;
        }
        appendLogLine(event);
      },
    });
    logStream = handle;
  }

  function closeStream() {
    if (logStream) {
      logStream.close();
      logStream = null;
    }
  }

  /// 日志帧落地: `line` 是文件原样行; `rotated` / `unreadable` 是宿主如实提示
  /// (用 data-note 记下键名以便切语言时重译)。
  function appendLogLine(event) {
    const div = document.createElement("div");
    if (event.kind === "line") {
      div.className = "rn-log-line";
      div.textContent = event.text;
    } else {
      const key = event.kind === "unreadable" ? "logUnreadable" : "rotated";
      div.className = "rn-log-note" + (event.kind === "unreadable" ? " unreadable" : "");
      div.dataset.note = key;
      if (event.kind === "unreadable") div.dataset.reason = event.text;
      div.textContent = div.dataset.reason ? t(key) + ": " + div.dataset.reason : t(key);
    }
    logLines.appendChild(div);
    while (logLines.childElementCount > LOG_DOM_MAX) {
      logLines.removeChild(logLines.firstChild);
    }
    logLines.scrollTop = logLines.scrollHeight;
  }

  /// 把持久面板插到展开行之后(轮询重画表体后调用; 行没了就收起)。
  function insertLogBox() {
    if (!expandedName) return;
    ensureLogBox();
    const tbody = $("rn-tbody");
    let anchor = null;
    for (const tr of tbody.querySelectorAll("tr.rn-row")) {
      if (tr.dataset.name === expandedName) {
        anchor = tr;
        break;
      }
    }
    if (!anchor) {
      collapseLog();
      return;
    }
    logTr = document.createElement("tr");
    logTr.className = "rn-log-tr";
    const td = document.createElement("td");
    td.colSpan = NUM_COLS;
    td.appendChild(logBox);
    logTr.appendChild(td);
    anchor.after(logTr);
  }

  // ---------- 弹窗 ----------
  //
  // 059 UX-09: 本地这份 `openModal` / `modalActions` 已删 —— 实现统一收进 `common.js` 的
  // `R.modal`(挂载点 `#modal-root`), 与策略视图共用同一份。原先那份缺 `role="dialog"` /
  // 焦点管理, 且**点取消关闭时 Esc 监听不摘**。这里只留薄封装。
  const openModal = (...a) => R.modal.open(...a);
  const modalActions = (...a) => R.modal.actions(...a);

  // ---------- 启动对话框(dry_run / demo / live 三模式; live 两步门禁) ----------

  async function openStartModal(preselect) {
    // 先取策略清单(失败就地报错, 不弹空窗)。
    let list;
    try {
      list = await R.api("/api/strategies");
    } catch (err) {
      setListMsg(t("rnLoadFailed") + ((err && err.message) || String(err)), "err");
      return;
    }
    if (!list.length) {
      setListMsg(t("rnNoStrategies"), "err");
      return;
    }
    // 清单取回后无需代际校验(弹窗自会随 deactivate 一起销毁)。
    buildStartModal(preselect, list);
  }

  function buildStartModal(preselect, list) {
    let alive = true;
    const g = gen; // 弹窗内的在途请求回来时, 视图若已离开(g 变了)就不再碰 DOM。
    const { card, close } = openModal(t("rnStartTitle"));
    const doClose = () => {
      alive = false;
      close();
    };

    const form = h("div", "sg-modal-form");

    // 063: 字段序对齐 fmz 的「新建实盘」页(/m/add-robot):
    //   实盘名称 → 运行策略(可搜索) → 策略参数 → 交易配置(交易对) → 模式。
    //
    // **实盘名称**: ricow 的实例名 = 策略 id(既有语义, 改它是另一个变更)。
    // 所以这个字段**只读展示**策略名, 不让用户改 —— 给了能改的输入框却改不动,
    // 比不给这个字段更让人困惑。
    const nameLabel = h("label", "sg-field");
    nameLabel.appendChild(h("span", null, t("rnLiveName")));
    const nameView = h("div", "rn-live-name", "");
    nameLabel.appendChild(nameView);
    form.appendChild(nameLabel);

    // 运行策略: **可搜索**下拉(fmz 形态)。一个输入框 + 一个结果列表 ——
    // 原生 `<select>` 没法搜索, 策略一多就找不着。
    const stratLabel = h("label", "sg-field");
    stratLabel.appendChild(h("span", null, t("rnStartStrategy")));
    const searchWrap = h("div", "rn-strat-search");
    const searchInput = document.createElement("input");
    searchInput.className = "sg-input";
    searchInput.type = "search";
    searchInput.autocomplete = "off";
    searchInput.placeholder = t("rnStratSearchPh");
    const resultBox = h("div", "rn-strat-results");
    const sel = document.createElement("select");
    sel.className = "sg-input rn-strat-hidden";
    sel.setAttribute("aria-hidden", "true");
    searchWrap.appendChild(searchInput);
    searchWrap.appendChild(resultBox);
    searchWrap.appendChild(sel);
    stratLabel.appendChild(searchWrap);
    form.appendChild(stratLabel);

    // 选中策略后展开的参数区(fmz 的「策略参数」折叠区)。
    const paramBox = h("div", "rn-live-params");
    paramBox.hidden = true;
    form.appendChild(paramBox);

    // 交易配置(fmz 的「交易配置」段): 交易对。
    const pairLabel = h("label", "sg-field");
    pairLabel.appendChild(h("span", null, t("rnLivePair")));
    const pairInput = document.createElement("input");
    pairInput.className = "sg-input";
    pairInput.type = "text";
    pairInput.autocomplete = "off";
    pairInput.value = "";
    pairLabel.appendChild(pairInput);
    form.appendChild(pairLabel);

    // 模式单选(dry_run 默认)。控件命名统一避开前端禁词扫描(见 web/mod.rs 扫描测试)。
    const modesLabel = h("label", "sg-field");
    modesLabel.appendChild(h("span", null, t("rnStartMode")));
    const modesBox = h("div", "rn-modes");
    const radios = [];
    for (const mode of ["dry_run", "demo", "live"]) {
      const item = h("label", "rn-mode-item");
      const radio = document.createElement("input");
      radio.type = "radio";
      radio.name = "rn-start-mode";
      radio.value = mode;
      if (mode === "dry_run") radio.checked = true;
      radio.addEventListener("change", onModeChange);
      item.appendChild(radio);
      item.appendChild(h("span", null, t(MODE_KEYS[mode])));
      modesBox.appendChild(item);
      radios.push(radio);
    }
    modesLabel.appendChild(modesBox);
    form.appendChild(modesLabel);

    // live: 逐字短语输入区(默认隐藏; 期望短语原文明示, 随所选策略实时更新)。
    const liveWrap = h("div", "sg-field");
    liveWrap.hidden = true;
    const liveHint = h("div", "sg-field-hint");
    liveWrap.appendChild(liveHint);
    const phraseEl = document.createElement("input");
    phraseEl.className = "sg-input";
    phraseEl.type = "text";
    phraseEl.autocomplete = "off";
    phraseEl.spellcheck = false;
    liveWrap.appendChild(phraseEl);
    form.appendChild(liveWrap);

    // demo: 缺测试网密钥时的置灰提示 + 去设置(默认隐藏)。
    const demoHint = h("div", "sg-msg err");
    demoHint.hidden = true;
    form.appendChild(demoHint);

    // 首次 live 风险披露(服务端 403 need_risk_ack 后展开; 默认隐藏)。
    const ackWrap = h("div", "rn-ack");
    ackWrap.hidden = true;
    ackWrap.appendChild(h("div", "rn-ack-title", t("rnAckTitle")));
    const disclosure = h("pre", "rn-disclosure");
    ackWrap.appendChild(disclosure);
    ackWrap.appendChild(h("div", "sg-field-hint", t("rnAckPhraseHint")));
    const ackRow = h("div", "rn-ack-row");
    const ackInput = document.createElement("input");
    ackInput.className = "sg-input";
    ackInput.type = "text";
    ackInput.autocomplete = "off";
    ackRow.appendChild(ackInput);
    const ackGo = h("button", "sg-btn", t("rnAckGo"));
    ackGo.type = "button";
    ackRow.appendChild(ackGo);
    ackWrap.appendChild(ackRow);
    form.appendChild(ackWrap);

    const errLine = h("div", "sg-msg err");
    form.appendChild(errLine);
    card.appendChild(form);

    const { cancel, primary } = modalActions(card, "rnStartGo");
    cancel.addEventListener("click", doClose);

    function currentMode() {
      for (const radio of radios) if (radio.checked) return radio.value;
      return "dry_run";
    }

    // ---- 063: 运行策略的可搜索选择 + 参数加载 ----
    //
    // fmz 的「运行策略」是一个**带搜索的下拉**: 选中后展开该策略的参数区。
    // 原生 `<select>` 不能搜索, 策略一多就找不着 —— 故自己做一个搜索框 + 结果列表,
    // 底下藏一个真 `<select>` 承接值(表单语义与既有代码都不必改)。

    /// 当前选中的策略 id(藏在真 select 里)。
    function currentStrategy() {
      return sel.value;
    }

    /// 渲染搜索结果(按名称/关键词过滤; 空关键词时列全部)。
    function renderResults() {
      const kw = searchInput.value.trim().toLowerCase();
      resultBox.innerHTML = "";
      const hits = list.filter((s) => {
        if (!kw) return true;
        return (
          (s.name || "").toLowerCase().includes(kw) ||
          (s.id || "").toLowerCase().includes(kw) ||
          (s.summary || "").toLowerCase().includes(kw)
        );
      });
      if (!hits.length) {
        resultBox.appendChild(h("div", "rn-strat-none", t("rnStratNoHit")));
        return;
      }
      for (const s of hits) {
        const item = h("button", "rn-strat-item");
        item.type = "button";
        item.appendChild(h("span", "rn-strat-item-name", s.name || s.id));
        const src = h("span", "rn-strat-item-src", s.source === "builtin" ? t("sgBuiltin") : t("sgMine"));
        item.appendChild(src);
        item.addEventListener("click", () => {
          // 隐藏 select 里**必须真的有匹配的 option** —— 浏览器对"不存在的 value"
          // 会把 select.value 置成空串(规范如此), 于是后面 currentStrategy() 取到 "",
          // 提交时按"没选策略"提前返回, **参数一个都没落盘**(症状: 弹窗关了但参数没生效)。
          // 故先补 option 再赋值。
          let opt = Array.prototype.slice.call(sel.options).find(function (o) {
            return o.value === s.id;
          });
          if (!opt) {
            opt = document.createElement("option");
            opt.value = s.id;
            opt.textContent = s.name || s.id;
            sel.appendChild(opt);
          }
          sel.value = s.id;
          nameView.textContent = s.name || s.id;
          searchInput.value = "";
          resultBox.innerHTML = "";
          searchInput.placeholder = s.name || s.id;
          syncBuiltinNote(s);
          loadParams(s);
          if (currentMode() === "live") syncLiveHint();
        });
        resultBox.appendChild(item);
      }
    }

    /// 选中策略后拉它的清单与当前值, 画参数表单。
    async function loadParams(s) {
      paramBox.innerHTML = "";
      paramBox.hidden = false;
      const head = h("div", "rn-live-params-head");
      head.appendChild(h("span", "rn-live-params-title", t("rnLiveParams")));
      // 064: 「重置参数」(fmz 的新建实盘页底部有这两个入口之一)。
      //
      // **只回清单默认值, 不清空用户填的必填项**: 没有默认值的必填参数清空后,
      // 用户面对的是一个空表单 + 一句"必填"—— 那不如不重置。故规则是
      // "有默认值 → 回默认值; 无默认值 → 保持原值不动"(清空只在用户主动清时发生)。
      const reset = h("button", "sg-btn sg-btn-mini");
      reset.type = "button";
      reset.id = "rn-live-reset";
      reset.appendChild(h("span", null, t("rnLiveReset")));
      reset.addEventListener("click", () => {
        const S = R.sg;
        const entries = paramBox.__entries || [];
        // 只收**有默认值**的那些键: 无默认值的保持原值(见上面的说明)。
        const want = {};
        for (const e of entries) {
          if (e.def === undefined || e.def === null) continue;
          want[e.key] = e.def;
        }
        const n = S && typeof S.writeParamEntries === "function" ? S.writeParamEntries(entries, want) : 0;
        // **给一句反馈**: "恢复了几个" / "没有可恢复的"。
        // 不说的后果: 用户改的是一个**无默认值**的必填参数(如 start_price),
        // 点重置发现什么都没发生, 会以为按钮坏了(064 专项走查实测踩到这个误解)。
        const tip = h("div", "sg-hint");
        tip.textContent = n > 0
          ? (t("rnResetDone") || "").replace("{n}", String(n))
          : t("rnResetNothing");
        // 上一次的反馈先撤掉, 否则连点会摞一串。
        const old = paramBox.querySelector(".rn-live-reset-tip");
        if (old) old.remove();
        tip.classList.add("rn-live-reset-tip");
        paramBox.appendChild(tip);
      });
      head.appendChild(reset);
      paramBox.appendChild(head);
      let detail = null;
      let current = {};
      try {
        detail = await R.api("/api/strategies/" + encodeURIComponent(s.id));
        current = detail.current || {};
        if (detail.pair) pairInput.value = detail.pair;
        // 视野校验: 策略自带的交易对可能不在当前视野里(默认视野只有股票类)。
        // **不 await** —— 它是"锦上添花"的替换, 不该拖慢参数区渲染;
        // 拿不到视野时它什么都不做, 由服务端门禁如实报。
        syncPairScope(s, detail);
      } catch (err) {
        // 拉不到清单也要让用户能继续 —— 参数区如实说明, 不静默空白。
        const hint = h("div", "sg-hint", t("rnLiveParamsLoadFail") + " " + ((err && err.message) || String(err)));
        paramBox.appendChild(hint);
        return;
      }
      if (!alive || g !== gen) return;
      const S = R.sg;
      if (!S || !S.buildParamBlock) {
        paramBox.appendChild(h("div", "sg-hint", t("rnLiveParamsUnavailable")));
        return;
      }
      const built = S.buildParamBlock({ params: detail.params || [], current: current });
      if (!built.entries.length) {
        paramBox.appendChild(h("div", "sg-hint", t("rnLiveNoParams")));
        return;
      }
      paramBox.appendChild(built.box);
      // 收集器: 提交时从这些控件取值。
      paramBox.__entries = built.entries;
    }

    /// 从参数表单收集值(与详情页同口径: `S.readParam`)。
    function collectParams() {
      const S = R.sg;
      const entries = paramBox.__entries || [];
      const out = {};
      for (const e of entries) {
        const v = S.readParam(e);
        if (v !== undefined && v !== null) out[e.key] = v;
      }
      return out;
    }

    /// 内置策略的提示: 内置脚本编译期嵌入, 改不了 —— 新建实盘要**复制**成自己的。
    function syncBuiltinNote(s) {
      let note = paramBox.querySelector(".rn-live-builtin");
      if (!s || s.source !== "builtin") {
        if (note) note.remove();
        return;
      }
      if (!note) {
        note = h("div", "rn-live-builtin");
        paramBox.appendChild(note);
      }
      note.textContent = t("rnLiveBuiltinNote");
    }

    /// 063: **交易对视野校验** —— 策略自带的交易对可能不在**当前视野**里。
    ///
    /// 背景: 默认配置 `[market].show_all_pairs = false` 时视野只有股票类,
    /// 而策略的 pair 常是 `BTCUSDT`(复制内置策略时带过来的) —— 提交必然撞视野门禁。
    ///
    /// 为什么不干脆报错让用户自己改: 那是"提交 → 弹窗不关 → 一句门禁原文",
    /// 用户得先知道"视野"是什么才知道该改哪儿。**直接换成视野内的对 + 说明一句**
    /// 更省事, 也不丢事实(说明里写清"原值 → 现值")。
    ///
    /// 拿不到视野(离线/快照不可用)时**不动** —— 那时门禁自己会报, 不猜。
    async function syncPairScope(s, detail) {
      const want = detail && detail.pair;
      if (!want) return;
      let view = null;
      try {
        view = await R.api("/api/markets");
      } catch (err) {
        return;   // 拿不到视野就不猜, 交给门禁如实报
      }
      if (!view || !view.filtered) return;   // 视野没被过滤 → 什么都在范围内
      const inScope =
        (view.spot || []).some((x) => x === want) || (view.futures || []).some((x) => x === want);
      if (inScope) return;
      const fallback = (view.spot || [])[0] || (view.futures || [])[0];
      if (!fallback) return;
      pairInput.value = fallback;
      // **replaceAll**: `String.replace` 只替换**第一个**匹配 —— 文案里同一个占位符
      // 出现两次时第二次会原样留在界面上(2026-10-24 真机截图里就是这么发现的)。
      const hint = h(
        "div",
        "sg-hint",
        t("rnLivePairOutOfScope").replaceAll("{0}", want).replaceAll("{1}", fallback)
      );
      paramBox.appendChild(hint);
    }

    searchInput.addEventListener("input", renderResults);
    searchInput.addEventListener("focus", renderResults);
    // 点外面收起结果列表(不关弹窗)。
    document.addEventListener("click", (ev) => {
      if (!searchWrap.contains(ev.target)) resultBox.innerHTML = "";
    });

    /// live 期望短语(与后端同串: `确认实盘 <策略 id>`, 逐字)。
    function expectedPhrase(id) {
      return "确认实盘 " + id;
    }

    function syncLiveHint() {
      liveHint.textContent = t("rnPhraseHint") + expectedPhrase(currentStrategy());
    }

    function onModeChange() {
      const mode = currentMode();
      liveWrap.hidden = mode !== "live";
      demoHint.hidden = true;
      ackWrap.hidden = true;
      setMsg(errLine, "", "");
      primary.disabled = false;
      primary.textContent = t("rnStartGo");
      if (mode === "live") {
        syncLiveHint();
        phraseEl.focus();
      } else if (mode === "demo") {
        checkDemoKeys();
      }
    }

    /// demo 先查测试网密钥: 未配置则启动钮置灰 + 引导去密钥页。
    async function checkDemoKeys() {
      let cfg;
      try {
        cfg = await R.api("/api/config/keys");
      } catch (err) {
        if (!alive || g !== gen) return;
        setMsg(errLine, (err && err.message) || String(err), "err");
        return;
      }
      if (!alive || g !== gen) return;
      const ok = !!(
        cfg &&
        cfg.demo_key &&
        cfg.demo_key.configured &&
        cfg.demo_secret &&
        cfg.demo_secret.configured
      );
      if (ok) {
        primary.disabled = false;
        return;
      }
      primary.disabled = true;
      demoHint.innerHTML = "";
      demoHint.hidden = false;
      demoHint.appendChild(h("span", null, t("rnDemoKeysMissing") + " "));
      const go = h("button", "sg-btn sg-btn-mini", t("rnGoSettings"));
      go.type = "button";
      go.addEventListener("click", () => {
        doClose();
        R.navigate("keys");
      });
      demoHint.appendChild(go);
    }

    async function submit() {
      const name = currentStrategy();
      if (!name) {
        setMsg(errLine, t("rnNoStrategies"), "err");
        return;
      }
      const mode = currentMode();
      const body = { mode: mode };
      if (mode === "live") {
        const phrase = phraseEl.value;
        if (!phrase) {
          setMsg(errLine, t("rnPhraseEmpty"), "err");
          return;
        }
        body.phrase = phrase;
      }
      primary.disabled = true;
      primary.textContent = t("rnStarting");
      setMsg(errLine, "", "");

      // 063: **先落参数, 再启动** —— 这是"新建实盘能设参数"的落地顺序。
      // 落盘端点只改 params 与 pair, 不碰代码; 内置策略会 409(它改不了),
      // 那时如实提示"内置策略请先复制", 不静默跳过。
      const picked = list.find((s) => s.id === name);
      if (picked && picked.source === "builtin") {
        primary.disabled = false;
        primary.textContent = t("rnStartGo");
        setMsg(errLine, t("rnLiveBuiltinBlock"), "err");
        return;
      }
      try {
        const params = collectParams();
        const payload = { params: params };
        const pair = pairInput.value.trim();
        if (pair) payload.pair = pair;
        await R.api("/api/strategies/" + encodeURIComponent(name) + "/instance", {
          method: "POST",
          body: payload,
        });
      } catch (err) {
        if (!alive || g !== gen) return;
        primary.disabled = false;
        primary.textContent = t("rnStartGo");
        setMsg(errLine, (err && err.message) || String(err), "err");
        return;
      }
      if (!alive || g !== gen) return;

      let reply;
      try {
        reply = await R.api("/api/strategies/" + encodeURIComponent(name) + "/start", {
          method: "POST",
          body: body,
        });
      } catch (err) {
        if (!alive || g !== gen) return;
        primary.disabled = false;
        primary.textContent = t("rnStartGo");
        if (err && err.status === 403 && err.code === "need_risk_ack") {
          // 首次 live: 展示披露全文 + 要「确认风险」; 本轮启动请求零副作用。
          ackWrap.hidden = false;
          disclosure.textContent = (err && err.disclosure) || err.message || "";
          ackInput.value = "";
          ackInput.focus();
          return;
        }
        // 短语空/错(400) / 已在运行(409) / 缺凭据 / live 未开关: 服务端中文原文, 不关窗。
        // **资金不足也是这一类**: 引擎侧 `insufficient balance` 如实透出, 前端不拦
        // (用户明说"实盘如果资金不够执行会报错")。
        setMsg(errLine, (err && err.message) || String(err), "err");
        return;
      }
      if (!alive || g !== gen) return;
      void reply;
      doClose();
      setListMsg(t("rnStartOk") + " " + name, "ok");
      refreshQuiet(gen);
    }

    async function submitAck() {
      const phrase = ackInput.value;
      if (!phrase) {
        setMsg(errLine, t("rnPhraseEmpty"), "err");
        return;
      }
      ackGo.disabled = true;
      let ok = false;
      try {
        await R.api("/api/risk-ack", { method: "POST", body: { phrase: phrase } });
        ok = true;
      } catch (err) {
        if (alive && g === gen) setMsg(errLine, (err && err.message) || String(err), "err");
      }
      ackGo.disabled = false;
      if (!ok) return;
      if (!alive || g !== gen) return;
      // 确认已落盘: 收起披露区, 回到 live 短语步骤(下一次 start 不再 403)。
      ackWrap.hidden = true;
      setMsg(errLine, t("rnAckOk"), "ok");
      phraseEl.focus();
    }

    primary.addEventListener("click", () => submit().catch((e) => setMsg(errLine, (e && e.message) || String(e), "err")));
    ackGo.addEventListener("click", () => submitAck().catch((e) => setMsg(errLine, (e && e.message) || String(e), "err")));
    phraseEl.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") submit().catch((e) => setMsg(errLine, (e && e.message) || String(e), "err"));
    });
    ackInput.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") submitAck().catch((e) => setMsg(errLine, (e && e.message) || String(e), "err"));
    });
    sel.addEventListener("change", () => {
      if (currentMode() === "live") syncLiveHint();
    });

    // 063: 进来时按 preselect 预选(从策略页点"运行"进来时带着策略 id),
    // 并把它的参数与交易对载入 —— 用户改完就能直接启动, 不用再选一遍。
    const initial = preselect ? list.find((s) => s.id === preselect) : null;
    if (initial) {
      sel.value = initial.id;
      nameView.textContent = initial.name || initial.id;
      searchInput.placeholder = initial.name || initial.id;
      syncBuiltinNote(initial);
      loadParams(initial).catch((e) => setMsg(errLine, (e && e.message) || String(e), "err"));
    } else {
      // 没预选: 直接把搜索结果摊开 —— 用户第一眼就知道这里能选什么。
      renderResults();
    }
  }

  // ---------- 停止全部(kill switch, 058) ----------

  /// 期望短语**与后端同串**(web/runs/mod.rs `post_kill_all`): 勾了平仓就多「并平仓」三字。
  /// 这里是逐字比对, 不 trim、不接受包含 —— 少一个字都会被后端 400 打回, 与其那时才发现,
  /// 不如按钮直接 disabled 并把期望短语原文明示。
  function killPhrase(closeAll) {
    return "确认停止全部" + (closeAll ? "并平仓" : "");
  }

  /// 紧急按住全部运行中的策略(057 后端就绪, 058 补 Web 入口)。
  ///
  /// 为什么在弹窗里逐字输入而不是一个「确定」按钮: 这个动作**没有撤销**, 而且默认**只**停
  /// 策略、daemon 继续活着(紧急按住之后还要看现场), 与「停止单个」不是一回事。后端已用
  /// 逐字短语把这道门做实, 前端照做而不是绕过它。
  function openKillModal() {
    let alive = true;
    const g = gen;
    const { card, close } = openModal(t("rnKillTitle"));
    const doClose = () => {
      alive = false;
      close();
    };

    const form = h("div", "sg-modal-form");

    // 后果说明写在按钮之前(与策略删除确认同纪律): 用户按下之前必须先读到"会发生什么"。
    const body = h("div", "rn-kill-body");
    body.appendChild(h("div", "rn-kill-warn", t("rnKillWarn")));
    const keepLine = h("div", "rn-kill-keep");
    keepLine.appendChild(h("span", null, t("rnKillKeep")));
    keepLine.appendChild(h("span", "rn-kill-keep-daemon", t("rnKillKeepDaemon")));
    body.appendChild(keepLine);
    form.appendChild(body);

    // 勾选 = 连交易所挂单与持仓一起撤/平(不可逆动作, 默认不勾)。
    const checkLabel = h("label", "rn-stop-check");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    checkLabel.appendChild(cb);
    checkLabel.appendChild(h("span", null, t("rnKillCloseAll")));
    form.appendChild(checkLabel);

    // 逐字短语输入区(与实盘启动同一套写法: 期望短语原文明示, 不写对按钮不放行)。
    const phraseWrap = h("div", "sg-field");
    const phraseHint = h("div", "sg-field-hint");
    phraseWrap.appendChild(phraseHint);
    const phraseEl = document.createElement("input");
    phraseEl.className = "sg-input";
    phraseEl.type = "text";
    phraseEl.autocomplete = "off";
    phraseEl.spellcheck = false;
    phraseWrap.appendChild(phraseEl);
    form.appendChild(phraseWrap);

    const errLine = h("div", "sg-msg err");
    form.appendChild(errLine);
    card.appendChild(form);

    const { cancel, primary } = modalActions(card, "rnKillGo");
    cancel.addEventListener("click", doClose);

    function syncHint() {
      const want = killPhrase(cb.checked);
      phraseHint.textContent = t("rnPhraseHint") + want;
      // 前端只做「一致才放行」这一件事, 不代填 —— 确认必须由用户亲手逐字输入。
      primary.disabled = phraseEl.value !== want;
      if (phraseEl.value && !primary.disabled) errLine.textContent = "";
    }

    cb.addEventListener("change", () => {
      phraseEl.value = "";
      syncHint();
    });
    phraseEl.addEventListener("input", syncHint);
    phraseEl.addEventListener("keydown", (ev) => {
      if (ev.key !== "Enter") return;
      ev.preventDefault();
      if (primary.disabled) return;
      submit().catch((e) => setMsg(errLine, (e && e.message) || String(e), "err"));
    });

    async function submit() {
      primary.disabled = true;
      const label = primary.textContent;
      primary.textContent = t("rnStopping");
      let reply;
      try {
        reply = await R.api("/api/runs/kill", {
          method: "POST",
          body: { close_all: cb.checked, confirm: phraseEl.value },
        });
      } catch (err) {
        if (!alive || g !== gen) return;
        primary.textContent = label;
        syncHint();
        setMsg(errLine, (err && err.message) || String(err), "err");
        return;
      }
      if (!alive || g !== gen) return;
      doClose();
      // 报告口径照服务端原文透传(027 原则): stopped=0 时服务端只说"无需停止",
      // 前端不另加"已全部停止"这类头衔。
      const stopped = reply && typeof reply.stopped === "number" ? reply.stopped : null;
      const head = stopped === 0 ? t("rnKillNone") : t("rnKillOk") + stopped;
      const report = reply && reply.report ? " · " + reply.report : "";
      setListMsg(head + report, "ok");
      refreshQuiet(gen);
    }

    syncHint();
  }

  // ---------- 停止确认(可选同时撤单平仓) ----------

  function openStopModal(name) {
    let alive = true;
    const g = gen;
    const { card, close } = openModal(t("rnStopTitle"));
    const doClose = () => {
      alive = false;
      close();
    };

    const form = h("div", "sg-modal-form");
    const confirmLine = h("div", "rn-stop-confirm");
    confirmLine.appendChild(h("span", null, t("rnStopConfirm") + " "));
    confirmLine.appendChild(h("span", "rn-stop-name", name));
    confirmLine.appendChild(h("span", null, " ?"));
    form.appendChild(confirmLine);

    // 勾选 = 停机报告里附带撤单平仓(默认只停进程, 不碰交易所)。
    const checkLabel = h("label", "rn-stop-check");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    checkLabel.appendChild(cb);
    checkLabel.appendChild(h("span", null, t("rnStopCloseAll")));
    form.appendChild(checkLabel);

    const errLine = h("div", "sg-msg err");
    form.appendChild(errLine);
    card.appendChild(form);

    const { cancel, primary } = modalActions(card, "rnStopGo");
    cancel.addEventListener("click", doClose);

    async function submit() {
      primary.disabled = true;
      primary.textContent = t("rnStopping");
      let reply;
      try {
        reply = await R.api("/api/strategies/" + encodeURIComponent(name) + "/stop", {
          method: "POST",
          body: { close_all: cb.checked },
        });
      } catch (err) {
        if (!alive || g !== gen) return;
        primary.disabled = false;
        primary.textContent = t("rnStopGo");
        setMsg(errLine, (err && err.message) || String(err), "err");
        if (err && err.status === 400 && err.code === "not_running") {
          // 已经停了: 静默刷新一次, 让行状态跟上(提示文案用服务端原文)。
          refreshQuiet(gen);
        }
        return;
      }
      if (!alive || g !== gen) return;
      doClose();
      const report = reply && reply.report ? " · " + reply.report : "";
      setListMsg(t("rnStopOk") + " " + name + report, "ok");
      refreshQuiet(gen);
    }

    primary.addEventListener("click", () => submit().catch((e) => setMsg(errLine, (e && e.message) || String(e), "err")));
  }

  // ---------- 切语言: 动态文本就地刷新(静态标签由公共 applyLang 处理) ----------

  function refreshDynamic() {
    if (loadFailed) {
      renderLoadError(lastLoadErr, () => enter(gen));
    } else if (listData) {
      renderRows();
    }
    if (expandedName && logTitle) logTitle.textContent = t("rnLogOf") + " " + expandedName;
    if (logLines) {
      // 宿主提示行(rotated / unreadable)按记录的键名就地重译。
      for (const node of logLines.querySelectorAll("[data-note]")) {
        const reason = node.dataset.reason;
        node.textContent = reason ? t(node.dataset.note) + ": " + reason : t(node.dataset.note);
      }
    }
  }

  const baseApplyLang = R.applyLang;
  R.applyLang = function (lang) {
    const ret = baseApplyLang.call(this, lang);
    try {
      if (mounted) refreshDynamic();
    } catch (_) {
      // 视图尚未完成挂载时忽略。
    }
    return ret;
  };

  R.views.runs = {
    activate: () => {
      mount();
      active = true;
      const g = (gen += 1);
      R.applyLang(R.lang);
      // 059 UX-09: 弹窗挂载点统一为 `#modal-root`。
      R.modal.closeAll();
      enter(g);
      startPolling();
    },
    deactivate: () => {
      gen += 1; // 在途响应/轮询全部作废
      active = false;
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = 0;
      }
      collapseLog(); // 关日志流
      // 059 UX-09: 弹窗挂载点统一为 `#modal-root`(原 `rn-modal-root` 已从骨架移除)。
      R.modal.closeAll();
    },
  };
})();
