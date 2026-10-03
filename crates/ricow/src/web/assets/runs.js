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
    rnTitle: "运行",
    rnPollHint: "每 5 秒自动刷新;页面切到后台时暂停。",
    rnNoInstances: "暂无策略实例(到策略页保存实例配置后会出现在这里)。",
    rnNoStrategies: "还没有任何策略,先到策略页创建。",
    rnLoadFailed: "加载失败: ",
    rnRetry: "重试",
    rnLoading: "加载中…",
    rnThName: "名称",
    rnThMode: "模式",
    rnThStatus: "状态",
    rnThPair: "标的",
    rnThNet: "净收益",
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
    rnStartTitle: "启动策略",
    rnStartStrategy: "策略",
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
    rnStartGo: "启动",
    rnStarting: "启动中…",
    rnStartOk: "已启动: ",
    rnStopTitle: "停止策略",
    rnStopConfirm: "确认停止",
    rnStopCloseAll: "同时撤销全部挂单并平仓",
    rnStopGo: "停止",
    rnStopping: "停止中…",
    rnStopOk: "已停止: ",
    rnLogOf: "日志: ",
    rnLogClose: "收起",
    rnLogToggle: "点击展开/收起日志",
  });
  Object.assign(R.TEXT.en, {
    rnTitle: "Runs",
    rnPollHint: "Auto-refreshes every 5s; paused while the page is hidden.",
    rnNoInstances: "No strategy instances yet (save one in Strategies and it shows up here).",
    rnNoStrategies: "No strategies yet — create one in Strategies first.",
    rnLoadFailed: "Load failed: ",
    rnRetry: "Retry",
    rnLoading: "Loading…",
    rnThName: "Name",
    rnThMode: "Mode",
    rnThStatus: "Status",
    rnThPair: "Pair",
    rnThNet: "Net PnL",
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
    rnStartTitle: "Start strategy",
    rnStartStrategy: "Strategy",
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
    rnStartGo: "Start",
    rnStarting: "Starting…",
    rnStartOk: "Started: ",
    rnStopTitle: "Stop strategy",
    rnStopConfirm: "Confirm stop",
    rnStopCloseAll: "Also cancel all open orders and close positions",
    rnStopGo: "Stop",
    rnStopping: "Stopping…",
    rnStopOk: "Stopped: ",
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
      R.TEXT.zh.rnPollHint + "</span></div>" +
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
      "<th" + bi("rnThAction") + ">" + R.TEXT.zh.rnThAction + "</th>" +
      "</tr></thead>" +
      '<tbody id="rn-tbody"></tbody></table></div>' +
      "</div>" +
      '<div id="rn-modal-root"></div></div>';

    // 回前台立即补一次刷新(轮询在不可见期间是暂停的)。
    document.addEventListener("visibilitychange", onVisibility);
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
    if (!listData || !listData.length) {
      const tr = document.createElement("tr");
      tr.className = "rn-empty";
      const td = h("td", null, t("rnNoInstances"));
      td.colSpan = NUM_COLS;
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

  /// 秒数 → 紧凑时长; null → "--"。
  function fmtUptime(secs) {
    if (secs === null || secs === undefined) return "--";
    const s = Number(secs);
    const d = Math.floor(s / 86400);
    const hh = Math.floor((s % 86400) / 3600);
    const mm = Math.floor((s % 3600) / 60);
    const ss = s % 60;
    if (d > 0) return d + "d " + hh + "h";
    if (hh > 0) return hh + "h " + mm + "m";
    if (mm > 0) return mm + "m " + ss + "s";
    return ss + "s";
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

  // ---------- 通用弹窗(与策略页同款骨架) ----------

  function openModal(titleText) {
    const overlay = h("div", "sg-modal-overlay");
    const card = h("div", "sg-modal-card");
    card.appendChild(h("div", "sg-modal-title", titleText));
    overlay.appendChild(card);
    $("rn-modal-root").appendChild(overlay);
    const close = () => overlay.remove();
    overlay.addEventListener("mousedown", (ev) => {
      if (ev.target === overlay) close();
    });
    const onKey = (ev) => {
      if (ev.key === "Escape") {
        close();
        document.removeEventListener("keydown", onKey);
      }
    };
    document.addEventListener("keydown", onKey);
    return { card, close };
  }

  /// 弹窗底部一行: 取消 + 主钮; 返回 { cancel, primary }。
  function modalActions(card, primaryKey) {
    const row = h("div", "sg-modal-actions");
    const cancel = h("button", "sg-btn", t("sgCancel"));
    cancel.type = "button";
    const primary = h("button", "sg-btn sg-btn-primary", t(primaryKey));
    primary.type = "button";
    row.appendChild(cancel);
    row.appendChild(primary);
    card.appendChild(row);
    return { cancel, primary };
  }

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

    // 策略下拉: 现货 / 合约两组(与策略页同口径)。
    const stratLabel = h("label", "sg-field");
    stratLabel.appendChild(h("span", null, t("rnStartStrategy")));
    const sel = document.createElement("select");
    sel.className = "sg-input";
    for (const market of ["spot", "futures"]) {
      const group = list.filter((s) => s.market === market);
      if (!group.length) continue;
      const og = document.createElement("optgroup");
      og.label = market === "spot" ? t("spot") : t("futures");
      for (const s of group) {
        const opt = document.createElement("option");
        opt.value = s.id;
        opt.textContent = s.name;
        if (s.id === preselect) opt.selected = true;
        og.appendChild(opt);
      }
      sel.appendChild(og);
    }
    stratLabel.appendChild(sel);
    form.appendChild(stratLabel);

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

    /// live 期望短语(与后端同串: `确认实盘 <策略 id>`, 逐字)。
    function expectedPhrase(id) {
      return "确认实盘 " + id;
    }

    function syncLiveHint() {
      liveHint.textContent = t("rnPhraseHint") + expectedPhrase(sel.value);
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
      const name = sel.value;
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
      $("rn-modal-root").innerHTML = "";
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
      $("rn-modal-root").innerHTML = "";
    },
  };
})();
