// Ricow Web 前端「策略」视图 (032 US3, FR-015 ~ FR-021):
//   列表(市场 × 来源四组) / 复制内置 / 新建空白 / 详情(参数表单 + Lua 编辑器) /
//   保存(编译门禁, 运行中拒绝) / AI 改 Lua(草稿+撤销, need_keys 引导设置页) / 回测异步作业(轮询三态)。
// 注册为 `R.views.strategies`; 全部数据走 R.api 的 `/api/strategies*` 与 `/api/backtest*`。
//
// 过期响应: 视图级"代际令牌" gen —— activate/deactivate 自增, 在途响应回来比对捕获值, 不一致即丢弃。
// 安全: 不写浏览器持久存储; 服务端数据一律 textContent / .value(不拼 innerHTML); 编辑器不输出 HTML。
"use strict";

(function () {
  const R = window.Ricow;
  const t = (key) => R.t(key);

  // ---------- 双语文案(并入公共字典; 静态标签走 data-zh/data-en) ----------
  Object.assign(R.TEXT.zh, {
    sgTitle: "策略",
    sgNew: "+ 新建空白",
    sgCopy: "复制",
    sgOpen: "打开",
    sgBuiltin: "内置",
    sgUser: "我的",
    sgLoading: "加载中…",
    sgLoadFailed: "加载失败: ",
    sgRetry: "重试",
    sgSpotBuiltin: "现货 · 内置",
    sgSpotUser: "现货 · 我的",
    sgFutBuiltin: "合约 · 内置",
    sgFutUser: "合约 · 我的",
    sgBack: "‹ 返回策略列表",
    sgParams: "参数",
    sgPair: "交易对",
    sgRequired: "必填",
    sgOtherParams: "其他已保存参数",
    sgCode: "Lua 源码",
    sgSave: "保存",
    sgSaving: "保存中…",
    sgSaved: "已保存",
    sgSaveLocked: "运行中,停止后才能保存",
    sgReload: "重新载入",
    sgDirty: "有未保存的修改",
    sgAiDirty: "AI 草稿未保存,请检查后保存",
    sgUndo: "撤销",
    sgAi: "AI 修改",
    sgAiPh: "用一句话说明要改什么,例如:把网格间距改成 ATR 三倍",
    sgAiGo: "AI 修改",
    sgAiBusy: "AI 修改中…",
    sgAiOk: "AI 草稿已生成(通过编译),确认无误后点「保存」才会落盘。",
    sgNeedKeys: "AI 还没有可用密钥,先到设置页配置后再试。",
    sgGoSettings: "去设置页配置",
    sgBacktest: "回测",
    sgInterval: "K 线周期",
    sgDays: "天数",
    sgAdvanced: "高级项(可选)",
    sgFee: "手续费率",
    sgCash: "初始资金",
    sgLeverage: "杠杆",
    sgStartBt: "开始回测",
    sgStartingBt: "提交中…",
    sgBtRunning: "回测运行中…",
    sgBtDone: "回测完成",
    sgBtError: "回测失败",
    sgBtRetry: "重试",
    sgBtExpired: "作业已过期或不存在(结果只保留 5 分钟)。",
    sgBusy: "同一策略已有回测在运行,请等它结束后再发起。",
    sgModalNew: "新建空白策略",
    sgModalCopy: "复制为我的策略",
    sgName: "策略名",
    sgNameHint: "只能用英文 / 数字 / 短横线 / 下划线,1–24 字符",
    sgMarket: "市场",
    sgCancel: "取消",
    sgCreate: "创建",
    sgConfirmCopy: "创建副本",
    sgCopying: "创建中…",
    sgCreating: "创建中…",
    sgPairRequired: "交易对不能为空",
    sgNameInvalid: "名称只能用 A-Z a-z 0-9 _ -,长度 1–24",
    sgDaysInvalid: "天数必须是 1–3650 的整数",
    sgNumberInvalid: "必须填数字",
    sgBuiltinHint:
      "内置策略只能查看,不能直接修改。用列表里的「复制」另存为自己的策略后再编辑;下方回测可直接运行。",
    sgRequiredEmpty: "必填参数为空",
    sgCodeEmpty: "Lua 源码不能为空",
    sgCopyOf: "原策略",
    sgAiCompilePrefix: "AI 草稿未通过编译,编辑器内容没有替换:",
    sgCompilePrefix: "编译未通过:",
    sgNoSummary: "—",
  });
  Object.assign(R.TEXT.en, {
    sgTitle: "Strategies",
    sgNew: "+ New blank",
    sgCopy: "Copy",
    sgOpen: "Open",
    sgBuiltin: "Built-in",
    sgUser: "Mine",
    sgLoading: "Loading…",
    sgLoadFailed: "Load failed: ",
    sgRetry: "Retry",
    sgSpotBuiltin: "Spot · Built-in",
    sgSpotUser: "Spot · Mine",
    sgFutBuiltin: "Futures · Built-in",
    sgFutUser: "Futures · Mine",
    sgBack: "‹ Back to strategies",
    sgParams: "Parameters",
    sgPair: "Pair",
    sgRequired: "required",
    sgOtherParams: "Other saved parameters",
    sgCode: "Lua source",
    sgSave: "Save",
    sgSaving: "Saving…",
    sgSaved: "Saved",
    sgSaveLocked: "Running — stop it before saving",
    sgReload: "Reload",
    sgDirty: "Unsaved changes",
    sgAiDirty: "AI draft not saved — review then save",
    sgUndo: "Undo",
    sgAi: "AI edit",
    sgAiPh: "Say what to change in one sentence, e.g. triple the grid spacing via ATR",
    sgAiGo: "Edit with AI",
    sgAiBusy: "AI working…",
    sgAiOk: "AI draft ready (compiles). Press Save to write it to disk.",
    sgNeedKeys: "No AI key is configured yet. Set one up in Settings, then try again.",
    sgGoSettings: "Go to Settings",
    sgBacktest: "Backtest",
    sgInterval: "Interval",
    sgDays: "Days",
    sgAdvanced: "Advanced (optional)",
    sgFee: "Fee rate",
    sgCash: "Initial cash",
    sgLeverage: "Leverage",
    sgStartBt: "Run backtest",
    sgStartingBt: "Submitting…",
    sgBtRunning: "Backtest running…",
    sgBtDone: "Backtest finished",
    sgBtError: "Backtest failed",
    sgBtRetry: "Retry",
    sgBtExpired: "Job expired or unknown (results are kept for only 5 minutes).",
    sgBusy: "A backtest of this strategy is already running. Wait for it to finish.",
    sgModalNew: "New blank strategy",
    sgModalCopy: "Copy as my strategy",
    sgName: "Name",
    sgNameHint: "Letters / digits / dash / underscore only, 1–24 chars",
    sgMarket: "Market",
    sgCancel: "Cancel",
    sgCreate: "Create",
    sgConfirmCopy: "Create copy",
    sgCopying: "Creating…",
    sgCreating: "Creating…",
    sgPairRequired: "Pair must not be empty",
    sgNameInvalid: "Only A-Z a-z 0-9 _ -, 1–24 chars",
    sgDaysInvalid: "Days must be an integer in 1–3650",
    sgNumberInvalid: "Must be a number",
    sgBuiltinHint:
      "Built-in strategies are view-only. Use Copy in the list to save your own copy before editing. Backtest below still works.",
    sgRequiredEmpty: "A required parameter is empty",
    sgCodeEmpty: "Lua source must not be empty",
    sgCopyOf: "From",
    sgAiCompilePrefix: "AI draft failed the compile gate; the editor was not changed:",
    sgCompilePrefix: "Compile failed:",
    sgNoSummary: "—",
  });

  // 常量(与后端白名单同口径)。
  const INTERVALS = ["1m", "5m", "15m", "1h", "4h", "1d"];
  const DEFAULT_INTERVAL = "1h";
  const DEFAULT_PAIR = "BTCUSDT";
  const NAME_RE = /^[A-Za-z0-9_-]{1,24}$/;
  const POLL_MS = 1000;

  /// 新建空白用的最小可编译骨架(编译门禁只校验语法; 真正运行所需参数由 Lua 内 fallback 决定)。
  const BLANK_LUA = [
    "-- Ricow 策略骨架(控制台「新建空白」生成,已通过编译门禁)",
    "-- on_tick 在每个主时钟收线后被引擎调用; 返回订单数组, 不动作时返回空表 {}.",
    "-- 需要下单时可返回, 例如:",
    '--   { { pair = pair, side = "buy", size = 0.001, order_type = "market" } }',
    "function on_tick(ctx)",
    "  local pair = ctx:config_str(\"pair\")",
    "  return {}",
    "end",
    "",
  ].join("\n");

  let mounted = false;
  // 代际令牌: 每次激活/离开自增, 过期响应一律不碰 DOM。
  let gen = 0;
  let listData = null; // 最近一次列表响应(切语言直接重画, 不重新取数)
  // 当前详情上下文: { id, detail, source, builtin }
  let detailCtx = null;
  // 参数输入注册表(每次渲染详情重建): { key, ty, required, input }。
  let paramInputs = [];
  let savedCode = ""; // 进入时/上次保存成功的代码(撤销目标)
  let aiDraft = false; // 编辑器当前是否装着未落盘的 AI 草稿
  let saveLocked = false; // 服务端 409 running 后锁定保存钮
  // 回测作业台账(模块变量, 不跨刷新): strategyId -> { jobId, status, report, error, body }。
  const jobs = new Map();
  let pollTimer = 0;

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

  // ---------- 一次性挂载(骨架只建一次; 数据每次激活重取) ----------

  function mount() {
    if (mounted) return;
    const host = $("view-strategies");
    host.innerHTML =
      '<div class="sg-wrap">' +
      // ---- 列表态 ----
      '<div id="sg-listview" class="sg-listview">' +
      '<div class="sg-head"><h2 class="sg-title"' + bi("sgTitle") + ">" + R.TEXT.zh.sgTitle + "</h2>" +
      '<button id="sg-new" class="sg-btn sg-btn-primary" type="button"' + bi("sgNew") + ">" +
      R.TEXT.zh.sgNew + "</button></div>" +
      '<div id="sg-list-msg" class="sg-msg" aria-live="polite"></div>' +
      '<div id="sg-list-body" class="sg-groups"></div></div>' +
      // ---- 详情态 ----
      '<div id="sg-detail" class="sg-detail" hidden>' +
      '<div class="sg-crumbs"><a href="javascript:void(0)" id="sg-back"' + bi("sgBack") + ">" +
      R.TEXT.zh.sgBack + "</a></div>" +
      '<div class="sg-detail-head"><h2 id="sg-d-name" class="sg-d-name"></h2>' +
      '<span id="sg-d-badges" class="sg-badges"></span></div>' +
      '<div id="sg-builtin-hint" class="sg-hint" hidden></div>' +
      '<div id="sg-d-loadmsg"></div>' +
      '<div class="sg-detail-grid">' +
      // 参数
      '<section class="sg-card"><div class="sg-card-head"><span' + bi("sgParams") + ">" +
      R.TEXT.zh.sgParams + "</span></div><div id='sg-params' class='sg-card-body'></div></section>" +
      // Lua 编辑器
      '<section class="sg-card sg-code-card"><div class="sg-card-head"><span' + bi("sgCode") + ">" +
      R.TEXT.zh.sgCode + "</span>" +
      '<span id="sg-dirty" class="sg-dirty" hidden><span id="sg-dirty-text"></span>' +
      '<button id="sg-undo" class="sg-btn sg-btn-mini" type="button"' + bi("sgUndo") + ">" +
      R.TEXT.zh.sgUndo + "</button>" +
      '<button id="sg-save-top" class="sg-btn sg-btn-mini sg-btn-primary" type="button"' +
      bi("sgSave") + ">" + R.TEXT.zh.sgSave + "</button></span></div>" +
      '<div id="sg-savemsg" class="sg-msg" aria-live="polite"></div>' +
      '<textarea id="sg-code" class="sg-code" spellcheck="false" autocomplete="off"></textarea>' +
      '<div class="sg-row-right"><button id="sg-save" class="sg-btn sg-btn-primary" type="button"' +
      bi("sgSave") + ">" + R.TEXT.zh.sgSave + "</button></div></section>" +
      // AI
      '<section class="sg-card" id="sg-ai-card"><div class="sg-card-head"><span' + bi("sgAi") + ">" +
      R.TEXT.zh.sgAi + "</span></div>" +
      '<textarea id="sg-ai-instr" class="sg-ai-instr" rows="3" spellcheck="false"' +
      ' data-zh-placeholder="' + R.TEXT.zh.sgAiPh + '" data-en-placeholder="' +
      R.TEXT.en.sgAiPh + '"></textarea>' +
      '<div class="sg-row-right"><button id="sg-ai-go" class="sg-btn" type="button"' +
      bi("sgAiGo") + ">" + R.TEXT.zh.sgAiGo + "</button></div>" +
      '<div id="sg-ai-msg" class="sg-msg" aria-live="polite"></div></section>' +
      // 回测
      '<section class="sg-card"><div class="sg-card-head"><span' + bi("sgBacktest") + ">" +
      R.TEXT.zh.sgBacktest + "</span></div><div class='sg-card-body'>" +
      '<div class="sg-bt-grid">' +
      "<label><span" + bi("sgPair") + ">" + R.TEXT.zh.sgPair + "</span>" +
      '<input id="sg-bt-pair" class="sg-input" type="text" autocomplete="off" /></label>' +
      "<label><span" + bi("sgInterval") + ">" + R.TEXT.zh.sgInterval + "</span>" +
      '<select id="sg-bt-interval" class="sg-input"></select></label>' +
      "<label><span" + bi("sgDays") + ">" + R.TEXT.zh.sgDays + "</span>" +
      '<input id="sg-bt-days" class="sg-input" type="number" min="1" max="3650" step="1" value="90" /></label>' +
      "</div>" +
      '<details class="sg-advanced"><summary' + bi("sgAdvanced") + ">" +
      R.TEXT.zh.sgAdvanced + "</summary><div class='sg-bt-grid'>" +
      "<label><span" + bi("sgFee") + ">" + R.TEXT.zh.sgFee + "</span>" +
      '<input id="sg-bt-fee" class="sg-input" type="number" step="any" /></label>' +
      "<label><span" + bi("sgCash") + ">" + R.TEXT.zh.sgCash + "</span>" +
      '<input id="sg-bt-cash" class="sg-input" type="number" step="any" /></label>' +
      "<label><span" + bi("sgLeverage") + ">" + R.TEXT.zh.sgLeverage + "</span>" +
      '<input id="sg-bt-leverage" class="sg-input" type="number" step="any" /></label>' +
      "</div></details>" +
      '<div class="sg-row-right"><button id="sg-bt-go" class="sg-btn sg-btn-primary" type="button"' +
      bi("sgStartBt") + ">" + R.TEXT.zh.sgStartBt + "</button></div>" +
      '<div id="sg-bt-msg" class="sg-msg" aria-live="polite"></div>' +
      '<div id="sg-bt-result"></div></div></section>' +
      "</div></div>" +
      // 弹窗层(新建/复制)
      '<div id="sg-modal-root"></div></div>';

    // 周期下拉选项(静态常量, 直接建 option)。
    const ivSel = $("sg-bt-interval");
    for (const iv of INTERVALS) {
      const opt = document.createElement("option");
      opt.value = iv;
      opt.textContent = iv;
      if (iv === DEFAULT_INTERVAL) opt.selected = true;
      ivSel.appendChild(opt);
    }

    $("sg-new").addEventListener("click", openNewModal);
    $("sg-back").addEventListener("click", () => R.navigate("strategies"));
    $("sg-code").addEventListener("keydown", onCodeKeydown);
    $("sg-code").addEventListener("input", () => updateDirty());
    $("sg-undo").addEventListener("click", onUndo);
    $("sg-save").addEventListener("click", onSave);
    $("sg-save-top").addEventListener("click", onSave);
    $("sg-ai-go").addEventListener("click", onAiEdit);
    $("sg-bt-go").addEventListener("click", onBacktest);
    mounted = true;
  }

  // ---------- 通用弹窗(内容由调用方用 DOM 追加; 文案随当前语言) ----------

  function openModal(titleText) {
    const overlay = h("div", "sg-modal-overlay");
    const card = h("div", "sg-modal-card");
    card.appendChild(h("div", "sg-modal-title", titleText));
    overlay.appendChild(card);
    $("sg-modal-root").appendChild(overlay);
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

  // ---------- 列表态 ----------

  async function enterList(g) {
    $("sg-list-body").innerHTML = "";
    $("sg-list-body").appendChild(h("div", "sg-loading", t("sgLoading")));
    setMsg($("sg-list-msg"), "", "");
    try {
      listData = await R.api("/api/strategies");
      if (g !== gen) return;
      renderList(listData);
    } catch (err) {
      if (g !== gen) return;
      renderListError(err, () => enterList(g));
    }
  }

  function renderListError(err, retry) {
    const body = $("sg-list-body");
    body.innerHTML = "";
    const box = h("div", "sg-error-box");
    box.appendChild(h("div", "sg-msg err", t("sgLoadFailed") + ((err && err.message) || "")));
    const btn = h("button", "sg-btn", t("sgRetry"));
    btn.type = "button";
    btn.addEventListener("click", retry);
    box.appendChild(btn);
    body.appendChild(box);
  }

  /// 四组: 市场(现货/合约) × 来源(内置/我的), 组序固定。
  function renderList(data) {
    const defs = [
      { labelKey: "sgSpotBuiltin", market: "spot", source: "builtin" },
      { labelKey: "sgSpotUser", market: "spot", source: "user" },
      { labelKey: "sgFutBuiltin", market: "futures", source: "builtin" },
      { labelKey: "sgFutUser", market: "futures", source: "user" },
    ];
    const body = $("sg-list-body");
    body.innerHTML = "";
    for (const def of defs) {
      const rows = data.filter((s) => s.market === def.market && s.source === def.source);
      const sec = h("section", "sg-group");
      const head = h("div", "sg-group-head");
      head.appendChild(h("span", null, t(def.labelKey)));
      head.appendChild(h("span", "sg-count", String(rows.length)));
      sec.appendChild(head);
      const box = h("div", "sg-rows");
      if (!rows.length) {
        box.appendChild(h("div", "sg-empty", "—"));
      } else {
        for (const row of rows) box.appendChild(renderRow(row));
      }
      sec.appendChild(box);
      body.appendChild(sec);
    }
  }

  function renderRow(s) {
    const row = h("div", "sg-row");
    const main = h("button", "sg-row-main");
    main.type = "button";
    main.appendChild(h("span", "sg-row-name", s.name));
    main.appendChild(
      h("span", "sg-row-summary", s.summary ? s.summary : t("sgNoSummary"))
    );
    const act = h("div", "sg-row-actions");
    if (s.source === "builtin") {
      // 内置: 只能复制后再改。
      const btn = h("button", "sg-btn sg-btn-mini", t("sgCopy"));
      btn.type = "button";
      btn.addEventListener("click", () => onCopy(s).catch(strategyErr));
      act.appendChild(btn);
      main.addEventListener("click", () => onCopy(s).catch(strategyErr));
    } else {
      // 用户: 打开详情编辑。
      const btn = h("button", "sg-btn sg-btn-mini", t("sgOpen"));
      btn.type = "button";
      btn.addEventListener("click", () => openDetail(s.id));
      act.appendChild(btn);
      main.addEventListener("click", () => openDetail(s.id));
    }
    row.appendChild(main);
    row.appendChild(act);
    return row;
  }

  function strategyErr(err) {
    setMsg($("sg-list-msg"), (err && err.message) || String(err), "err");
  }

  function openDetail(id) {
    R.navigate("strategies/" + encodeURIComponent(id));
  }

  // ---------- 新建空白 ----------

  function openNewModal() {
    const { card, close } = openModal(t("sgModalNew"));
    const form = h("div", "sg-modal-form");

    const nameLabel = h("label", "sg-field");
    nameLabel.appendChild(h("span", null, t("sgName")));
    const nameInput = document.createElement("input");
    nameInput.className = "sg-input";
    nameInput.type = "text";
    nameInput.autocomplete = "off";
    nameInput.spellcheck = false;
    nameLabel.appendChild(nameInput);
    nameLabel.appendChild(h("div", "sg-field-hint", t("sgNameHint")));
    form.appendChild(nameLabel);

    const marketLabel = h("label", "sg-field");
    marketLabel.appendChild(h("span", null, t("sgMarket")));
    const marketSel = document.createElement("select");
    marketSel.className = "sg-input";
    for (const [value, label] of [["spot", t("spot")], ["futures", t("futures")]]) {
      const opt = document.createElement("option");
      opt.value = value;
      opt.textContent = label;
      marketSel.appendChild(opt);
    }
    marketLabel.appendChild(marketSel);
    form.appendChild(marketLabel);

    const pairLabel = h("label", "sg-field");
    pairLabel.appendChild(h("span", null, t("sgPair")));
    const pairInput = document.createElement("input");
    pairInput.className = "sg-input";
    pairInput.type = "text";
    pairInput.value = DEFAULT_PAIR;
    pairInput.autocomplete = "off";
    pairLabel.appendChild(pairInput);
    form.appendChild(pairLabel);

    const errLine = h("div", "sg-msg err");
    form.appendChild(errLine);
    card.appendChild(form);
    const { cancel, primary } = modalActions(card, "sgCreate");
    cancel.addEventListener("click", close);

    async function submit() {
      const name = nameInput.value.trim();
      const pair = pairInput.value.trim();
      if (!NAME_RE.test(name)) {
        errLine.textContent = t("sgNameInvalid");
        return;
      }
      if (!pair) {
        errLine.textContent = t("sgPairRequired");
        return;
      }
      primary.disabled = true;
      primary.textContent = t("sgCreating");
      try {
        await R.api("/api/strategies", {
          method: "POST",
          body: {
            name: name,
            market: marketSel.value,
            pair: pair,
            code: BLANK_LUA,
            params: {},
            overwrite: false,
          },
        });
        close();
        openDetail(name);
      } catch (err) {
        // reserved/prefix/exists/compile/invalid_name 全部由服务端中文原文回填, 不关弹窗。
        errLine.textContent = (err && err.message) || String(err);
        primary.disabled = false;
        primary.textContent = t("sgCreate");
      }
    }
    primary.addEventListener("click", () => submit().catch(strategyErr));
    nameInput.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") submit().catch(strategyErr);
    });
    nameInput.focus();
  }

  // ---------- 复制内置 ----------

  /// 先取 Lua 原文(不弹空窗), 再开命名弹窗; 交易对缺省 BTCUSDT(内置无实例 TOML)。
  async function onCopy(s) {
    setMsg($("sg-list-msg"), t("sgLoading"), "");
    let src;
    try {
      src = await R.api("/api/strategies/" + encodeURIComponent(s.id) + "/source");
    } catch (err) {
      setMsg($("sg-list-msg"), (err && err.message) || String(err), "err");
      return;
    }
    setMsg($("sg-list-msg"), "", "");
    openCopyModal(s, src.lua);
  }

  /// 由内置 id 推一个合法的新名建议(短横线化后截到 24 字符)。
  function suggestName(id) {
    const base = id.replace(/[^A-Za-z0-9_-]/g, "-");
    const candidate = base + "-1";
    return candidate.length > 24 ? candidate.slice(0, 24) : candidate;
  }

  function openCopyModal(s, lua) {
    const { card, close } = openModal(t("sgModalCopy"));
    const form = h("div", "sg-modal-form");

    const from = h("div", "sg-copy-from");
    from.appendChild(h("span", "sg-muted", t("sgCopyOf") + ": "));
    from.appendChild(h("span", null, s.name));
    form.appendChild(from);

    const nameLabel = h("label", "sg-field");
    nameLabel.appendChild(h("span", null, t("sgName")));
    const nameInput = document.createElement("input");
    nameInput.className = "sg-input";
    nameInput.type = "text";
    nameInput.value = suggestName(s.id);
    nameInput.autocomplete = "off";
    nameLabel.appendChild(nameInput);
    nameLabel.appendChild(h("div", "sg-field-hint", t("sgNameHint")));
    form.appendChild(nameLabel);

    const pairLabel = h("label", "sg-field");
    pairLabel.appendChild(h("span", null, t("sgPair")));
    const pairInput = document.createElement("input");
    pairInput.className = "sg-input";
    pairInput.type = "text";
    pairInput.value = DEFAULT_PAIR;
    pairInput.autocomplete = "off";
    pairLabel.appendChild(pairInput);
    form.appendChild(pairLabel);

    const errLine = h("div", "sg-msg err");
    form.appendChild(errLine);
    card.appendChild(form);
    const { cancel, primary } = modalActions(card, "sgConfirmCopy");
    cancel.addEventListener("click", close);

    // 本地只做格式即时反馈(不发请求); 保留/前缀/重名由服务端 code 在保存时回填。
    nameInput.addEventListener("input", () => {
      errLine.textContent = NAME_RE.test(nameInput.value.trim())
        ? ""
        : t("sgNameInvalid");
    });

    async function submit() {
      const name = nameInput.value.trim();
      const pair = pairInput.value.trim();
      if (!NAME_RE.test(name)) {
        errLine.textContent = t("sgNameInvalid");
        return;
      }
      if (!pair) {
        errLine.textContent = t("sgPairRequired");
        return;
      }
      primary.disabled = true;
      primary.textContent = t("sgCopying");
      try {
        await R.api("/api/strategies", {
          method: "POST",
          // 副本是全新用户策略: 原样复制 Lua, 参数留空(用 Lua 内 fallback 默认), overwrite=false。
          body: {
            name: name,
            market: s.market,
            pair: pair,
            code: lua,
            params: {},
            overwrite: false,
          },
        });
        close();
        openDetail(name);
      } catch (err) {
        errLine.textContent = (err && err.message) || String(err);
        primary.disabled = false;
        primary.textContent = t("sgConfirmCopy");
      }
    }
    primary.addEventListener("click", () => submit().catch(strategyErr));
    nameInput.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") submit().catch(strategyErr);
    });
    nameInput.focus();
    nameInput.select();
  }

  // ---------- 详情态 ----------

  async function enterDetail(id, g) {
    detailCtx = null;
    paramInputs = [];
    savedCode = "";
    aiDraft = false;
    saveLocked = false;
    $("sg-params").innerHTML = "";
    $("sg-code").value = "";
    $("sg-code").readOnly = false;
    $("sg-ai-instr").value = "";
    setMsg($("sg-savemsg"), "", "");
    setMsg($("sg-ai-msg"), "", "");
    setMsg($("sg-bt-msg"), "", "");
    $("sg-bt-result").innerHTML = "";
    $("sg-dirty").hidden = true;
    $("sg-builtin-hint").hidden = true;
    syncSaveBtn("idle");
    syncAiBtn(false);
    syncBtBtn(false);

    const loadBox = $("sg-d-loadmsg");
    loadBox.innerHTML = "";
    loadBox.appendChild(h("div", "sg-loading", t("sgLoading")));
    $("sg-d-name").textContent = id;

    async function load() {
      const [detail, source] = await Promise.all([
        R.api("/api/strategies/" + encodeURIComponent(id)),
        R.api("/api/strategies/" + encodeURIComponent(id) + "/source"),
      ]);
      return { detail, source };
    }

    try {
      const { detail, source } = await load();
      if (g !== gen) return;
      loadBox.innerHTML = "";
      renderDetail(id, detail, source, g);
    } catch (err) {
      if (g !== gen) return;
      loadBox.innerHTML = "";
      const box = h("div", "sg-error-box");
      box.appendChild(h("div", "sg-msg err", (err && err.message) || String(err)));
      const btn = h("button", "sg-btn", t("sgRetry"));
      btn.type = "button";
      btn.addEventListener("click", () => enterDetail(id, g));
      box.appendChild(btn);
      loadBox.appendChild(box);
    }
  }

  /// 数字/布尔在 JSON 里都是基本类型; 据此决定额外行用哪种控件。
  function extraInputFor(value) {
    if (typeof value === "boolean") return { kind: "bool" };
    if (typeof value === "number") {
      return { kind: Number.isInteger(value) ? "i64" : "f64" };
    }
    return { kind: "string" };
  }

  function renderDetail(id, detail, source, g) {
    detailCtx = { id: id, detail: detail, source: source, builtin: source.instance_toml === null && detail.current === null && detail.pair === null };
    // 注: 内置判定以 source 响应的 instance_toml=null 为准(用户策略理论上恒有实例文件)。
    detailCtx.builtin = source.instance_toml === null;

    $("sg-d-name").textContent = detail.name || id;
    renderBadges(detail, source);

    // 内置只读提示 + 编辑/AI 控件收起(回测保留)。
    const builtin = detailCtx.builtin;
    $("sg-builtin-hint").hidden = !builtin;
    $("sg-builtin-hint").textContent = t("sgBuiltinHint");
    $("sg-code").readOnly = builtin;
    $("sg-save").hidden = builtin;
    $("sg-save-top").hidden = builtin;
    $("sg-ai-card").style.display = builtin ? "none" : "";

    renderParams(detail);
    $("sg-pair").value = detail.pair || DEFAULT_PAIR;

    // savedCode 必须取 textarea getter 规范化后的值(CRLF→LF): 直接存服务端原文会让
    // CRLF 策略(Windows 全部)在 updateDirty 里恒判 dirty(「有未保存的修改」常亮)。
    $("sg-code").value = source.lua || "";
    savedCode = $("sg-code").value;
    aiDraft = false;
    updateDirty();
    setMsg($("sg-savemsg"), "", "");

    // 回测表单 pair 跟随当前实例, 周期/天数保留用户上次输入。
    if (!$("sg-bt-pair").value) $("sg-bt-pair").value = detail.pair || DEFAULT_PAIR;

    // 恢复该策略的在途/近期作业(模块台账, 不跨刷新)。
    const job = jobs.get(id);
    if (job) {
      if (job.status === "running") startPolling(id, job.jobId, g);
      renderJob(job);
    }
  }

  function renderBadges(detail, source) {
    const box = $("sg-d-badges");
    box.innerHTML = "";
    box.appendChild(
      h("span", "sg-badge", detail.market === "futures" ? t("futures") : t("spot"))
    );
    box.appendChild(
      h("span", "sg-badge", source.instance_toml === null ? t("sgBuiltin") : t("sgUser"))
    );
  }

  // ---------- 参数表单 ----------

  function paramRow(spec, value) {
    const label = h("label", "sg-param");
    const head = h("span", "sg-param-head");
    head.appendChild(h("span", "sg-param-name", spec.name || spec.key));
    head.appendChild(h("span", "sg-param-key", spec.key));
    if (spec.required) head.appendChild(h("span", "sg-req", "* " + t("sgRequired")));
    label.appendChild(head);

    // 局部控件统一命名 ctl: 前端禁词扫描不允许给名为 input 的变量做点号 type 赋值(见 web/mod.rs 扫描测试)。
    let ctl;
    if (spec.ty === "bool") {
      ctl = document.createElement("input");
      ctl.type = "checkbox";
      ctl.checked = value === true;
      const wrap = h("div", "sg-param-check");
      wrap.appendChild(ctl);
      label.appendChild(wrap);
    } else if (spec.ty === "enum") {
      ctl = document.createElement("select");
      ctl.className = "sg-input";
      // 空选项(用默认值时不覆盖)。
      const blank = document.createElement("option");
      blank.value = "";
      blank.textContent = "—";
      ctl.appendChild(blank);
      for (const opt of spec.options || []) {
        const o = document.createElement("option");
        o.value = opt;
        o.textContent = opt;
        if (value === opt) o.selected = true;
        ctl.appendChild(o);
      }
      label.appendChild(ctl);
    } else {
      ctl = document.createElement("input");
      ctl.className = "sg-input";
      ctl.type = "number";
      ctl.step = spec.ty === "i64" ? "1" : "any";
      if (spec.ty === "i64") ctl.min = "0";
      if (value !== undefined && value !== null) ctl.value = String(value);
      label.appendChild(ctl);
    }
    if (spec.desc) label.appendChild(h("div", "sg-param-desc", spec.desc));
    return { label: label, input: ctl };
  }

  /// string 类型单独一个分支(text 输入); 与 number 分开避免拼错 type 属性。
  function stringRow(spec, value) {
    const label = h("label", "sg-param");
    const head = h("span", "sg-param-head");
    head.appendChild(h("span", "sg-param-name", spec.name || spec.key));
    head.appendChild(h("span", "sg-param-key", spec.key));
    if (spec.required) head.appendChild(h("span", "sg-req", "* " + t("sgRequired")));
    label.appendChild(head);
    const ctl = document.createElement("input");
    ctl.className = "sg-input";
    ctl.type = "text";
    ctl.autocomplete = "off";
    if (value !== undefined && value !== null) ctl.value = String(value);
    label.appendChild(ctl);
    if (spec.desc) label.appendChild(h("div", "sg-param-desc", spec.desc));
    return { label: label, input: ctl };
  }

  function renderParams(detail) {
    paramInputs = [];
    const box = $("sg-params");
    box.innerHTML = "";

    const pairField = h("label", "sg-param");
    pairField.appendChild(h("span", "sg-param-head", t("sgPair")));
    const pairInput = document.createElement("input");
    pairInput.className = "sg-input";
    pairInput.id = "sg-pair";
    pairInput.type = "text";
    pairInput.autocomplete = "off";
    pairField.appendChild(pairInput);
    box.appendChild(pairField);

    const current = detail.current || {};
    const known = new Set();

    for (const p of detail.params || []) {
      known.add(p.key);
      const built = p.ty === "string" ? stringRow(p, current[p.key] !== undefined ? current[p.key] : p.default)
        : paramRow(p, current[p.key] !== undefined ? current[p.key] : p.default);
      box.appendChild(built.label);
      paramInputs.push({ key: p.key, ty: p.ty, required: !!p.required, input: built.input });
    }

    // 实例里存在但清单没声明的键(网页保存的 lua 策略没有清单): 也给可编辑行, 避免保存时黑箱丢失
    // (即使不渲染, 保存也会以 current 为基线保留; 这里让人能改)。
    const extra = Object.keys(current)
      .filter((k) => !known.has(k))
      .sort();
    if (extra.length) {
      box.appendChild(h("div", "sg-params-subhead", t("sgOtherParams")));
      for (const key of extra) {
        const spec0 = { key: key, name: key, ty: extraInputFor(current[key]).kind, desc: "", required: false, options: null };
        const built = spec0.ty === "string" ? stringRow(spec0, current[key]) : paramRow(spec0, current[key]);
        box.appendChild(built.label);
        paramInputs.push({ key: key, ty: spec0.ty, required: false, input: built.input });
      }
    }
  }

  /// 读一个参数控件: 空 number/string → undefined(不覆盖); 非法数字抛错(带参数名)。
  function readParam(p) {
    const el = p.input;
    if (p.ty === "bool") return el.checked;
    const raw = el.value.trim();
    if (raw === "") return undefined;
    if (p.ty === "string" || p.ty === "enum") return raw;
    const n = Number(raw);
    if (!Number.isFinite(n)) throw new Error((p.key + ": ") + t("sgNumberInvalid"));
    if (p.ty === "i64") {
      if (!Number.isInteger(n)) throw new Error((p.key + ": ") + t("sgNumberInvalid"));
      return n;
    }
    return n;
  }

  /// 收集参数: 以实例 current 为基线(保住清单未声明的键), 表单值覆盖。
  /// requireFilled=true(保存)时校验必填; false(回测)时空值跳过。
  function gatherParams(requireFilled) {
    const out = {};
    if (detailCtx && detailCtx.detail.current) Object.assign(out, detailCtx.detail.current);
    for (const p of paramInputs) {
      const v = readParam(p);
      if (v === undefined) {
        if (requireFilled && p.required && p.ty !== "bool") {
          throw new Error(t("sgRequiredEmpty") + ": " + p.key);
        }
        continue;
      }
      out[p.key] = v;
    }
    return out;
  }

  // ---------- 编辑器脏状态 / Tab / 撤销 ----------

  function updateDirty() {
    const codeEl = $("sg-code");
    const dirty = codeEl.value !== savedCode;
    $("sg-dirty").hidden = !dirty;
    $("sg-dirty-text").textContent = aiDraft ? t("sgAiDirty") : t("sgDirty");
  }

  function onCodeKeydown(ev) {
    if (ev.key !== "Tab") return;
    // Tab 插入两空格并保持选区, 不跳焦(代码编辑器基本手感)。
    ev.preventDefault();
    const el = ev.target;
    const start = el.selectionStart;
    const end = el.selectionEnd;
    el.value = el.value.slice(0, start) + "  " + el.value.slice(end);
    el.selectionStart = el.selectionEnd = start + 2;
    updateDirty();
  }

  function onUndo() {
    $("sg-code").value = savedCode;
    aiDraft = false;
    setMsg($("sg-savemsg"), "", "");
    updateDirty();
  }

  // ---------- 保存 ----------

  function syncSaveBtn(mode) {
    for (const btn of [$("sg-save"), $("sg-save-top")]) {
      if (!btn) continue;
      if (mode === "saving") {
        btn.disabled = true;
        btn.textContent = t("sgSaving");
      } else if (mode === "locked") {
        btn.disabled = true;
        btn.textContent = t("sgSaveLocked");
      } else {
        btn.disabled = false;
        btn.textContent = t("sgSave");
      }
    }
  }

  function compileText(err) {
    // 编译错误: 服务端给 chunk 行号时定位到"第 N 行", 原文(中文 mlua 输出)照贴。
    const where = typeof err.line === "number" ? "第 " + err.line + " 行: " : "";
    return where + ((err && err.message) || "");
  }

  async function onSave() {
    if (!detailCtx || saveLocked) return;
    const code = $("sg-code").value;
    const pair = $("sg-pair").value.trim();
    if (!pair) {
      setMsg($("sg-savemsg"), t("sgPairRequired"), "err");
      return;
    }
    if (!code.trim()) {
      setMsg($("sg-savemsg"), t("sgCodeEmpty"), "err");
      return;
    }
    let params;
    try {
      params = gatherParams(true);
    } catch (err) {
      setMsg($("sg-savemsg"), err.message, "err");
      return;
    }

    syncSaveBtn("saving");
    setMsg($("sg-savemsg"), "", "");
    try {
      // 详情里的保存恒为覆盖自己(用户策略); 编译门禁 + 备份都在服务端。
      await R.api("/api/strategies", {
        method: "POST",
        body: {
          name: detailCtx.id,
          market: detailCtx.detail.market,
          pair: pair,
          code: code,
          params: params,
          overwrite: true,
        },
      });
      savedCode = code;
      aiDraft = false;
      updateDirty();
      setMsg($("sg-savemsg"), t("sgSaved"), "ok");
      syncSaveBtn("idle");
      refreshListQuietly();
    } catch (err) {
      syncSaveBtn("idle");
      if (err && err.code === "running") {
        // FR-020: 运行中禁止覆盖 —— 锁保存钮, 给重载入口(停止后回来自动恢复)。
        saveLocked = true;
        syncSaveBtn("locked");
        showRunningHint(err.message || t("sgSaveLocked"));
      } else if (err && err.code === "compile") {
        setMsg($("sg-savemsg"), t("sgCompilePrefix") + " " + compileText(err), "err");
      } else {
        setMsg($("sg-savemsg"), (err && err.message) || String(err), "err");
      }
    }
  }

  /// 运行中提示条: 服务端中文原因 + 重新载入钮(用户停止策略后点它解除锁定)。
  function showRunningHint(message) {
    const el = $("sg-savemsg");
    el.innerHTML = "";
    el.className = "sg-msg err";
    el.appendChild(h("span", null, message + " "));
    const reload = h("button", "sg-btn sg-btn-mini", t("sgReload"));
    reload.type = "button";
    reload.addEventListener("click", () => {
      saveLocked = false;
      const g = gen;
      enterDetail(detailCtx.id, g);
    });
    el.appendChild(reload);
  }

  /// 保存成功后静默刷新列表缓存(不改当前视图; 过期代际忽略)。
  function refreshListQuietly() {
    const g = gen;
    R.api("/api/strategies")
      .then((data) => {
        if (g === gen) listData = data;
      })
      .catch(() => {});
  }

  // ---------- AI 改 Lua ----------

  function syncAiBtn(busy) {
    const btn = $("sg-ai-go");
    btn.disabled = !!busy;
    btn.textContent = busy ? t("sgAiBusy") : t("sgAiGo");
  }

  async function onAiEdit() {
    if (!detailCtx) return;
    const instruction = $("sg-ai-instr").value.trim();
    if (!instruction) {
      setMsg($("sg-ai-msg"), t("sgAiPh"), "err");
      return;
    }
    const g = gen;
    syncAiBtn(true);
    setMsg($("sg-ai-msg"), t("sgAiBusy"), "");
    try {
      const reply = await R.api(
        "/api/strategies/" + encodeURIComponent(detailCtx.id) + "/ai-edit",
        { method: "POST", body: { instruction: instruction } }
      );
      if (g !== gen) return;
      // 成功: 服务端已过编译门禁; 替换编辑器, 撤销可回上次保存版本。
      $("sg-code").value = reply.code;
      aiDraft = true;
      updateDirty();
      setMsg($("sg-ai-msg"), t("sgAiOk"), "ok");
    } catch (err) {
      if (g !== gen) return;
      if (err && err.status === 403 && err.code === "need_keys") {
        showNeedKeys(err.message || t("sgNeedKeys"));
      } else if (err && err.code === "compile") {
        // 编译没过: 服务端没给可⽤代码, 编辑器内容**不替换**。
        setMsg($("sg-ai-msg"), t("sgAiCompilePrefix") + " " + compileText(err), "err");
      } else {
        setMsg($("sg-ai-msg"), (err && err.message) || String(err), "err");
      }
    } finally {
      if (g === gen) syncAiBtn(false);
    }
  }

  /// need_keys: 文案 + 一键去设置页(配置完用浏览器返回即可回到本视图)。
  function showNeedKeys(message) {
    const el = $("sg-ai-msg");
    el.innerHTML = "";
    el.className = "sg-msg err";
    el.appendChild(h("span", null, message + " "));
    const go = h("button", "sg-btn sg-btn-mini", t("sgGoSettings"));
    go.type = "button";
    go.addEventListener("click", () => R.navigate("settings"));
    el.appendChild(go);
  }

  // ---------- 回测作业 ----------

  function syncBtBtn(busy) {
    const btn = $("sg-bt-go");
    btn.disabled = !!busy;
    btn.textContent = busy ? t("sgStartingBt") : t("sgStartBt");
  }

  /// 读高级项: 空 → 不带键; 非法(NaN/负)带字段名报错。
  function optionalNumber(id, key, positive) {
    const raw = $(id).value.trim();
    if (raw === "") return undefined;
    const n = Number(raw);
    if (!Number.isFinite(n) || (positive ? n <= 0 : n < 0)) {
      throw new Error(key + ": " + t("sgNumberInvalid"));
    }
    return n;
  }

  async function onBacktest() {
    if (!detailCtx) return;
    const strategy = detailCtx.id;
    const pair = $("sg-bt-pair").value.trim() || $("sg-pair").value.trim();
    const interval = $("sg-bt-interval").value;
    const daysRaw = $("sg-bt-days").value.trim();
    const days = Number(daysRaw);
    if (!pair) {
      setMsg($("sg-bt-msg"), t("sgPairRequired"), "err");
      return;
    }
    if (!Number.isInteger(days) || days < 1 || days > 3650) {
      setMsg($("sg-bt-msg"), t("sgDaysInvalid"), "err");
      return;
    }
    let body;
    try {
      // 回测参数宽松收集(空值跳过; 是否真能跑由作业 error 态如实承载)。
      const params = gatherParams(false);
      body = { strategy: strategy, pair: pair, interval: interval, days: days, params: params };
      const fee = optionalNumber("sg-bt-fee", t("sgFee"), false);
      if (fee !== undefined) body.fee = fee;
      const cash = optionalNumber("sg-bt-cash", t("sgCash"), false);
      if (cash !== undefined) body.cash = cash;
      const leverage = optionalNumber("sg-bt-leverage", t("sgLeverage"), true);
      if (leverage !== undefined) body.leverage = leverage;
    } catch (err) {
      setMsg($("sg-bt-msg"), err.message, "err");
      return;
    }

    const g = gen;
    syncBtBtn(true);
    setMsg($("sg-bt-msg"), "", "");
    try {
      const reply = await R.api("/api/backtest", { method: "POST", body: body });
      if (g !== gen) return;
      const job = { jobId: reply.job_id, status: "running", report: null, error: null, body: body };
      jobs.set(strategy, job);
      renderJob(job);
      startPolling(strategy, job.jobId, g);
    } catch (err) {
      if (g !== gen) return;
      // 同名作业在跑 → 409 busy; 若台账里正好有它, 直接把在途作业画出来。
      if (err && err.status === 409 && err.code === "busy") {
        const existing = jobs.get(strategy);
        if (existing && existing.status === "running") {
          renderJob(existing);
          startPolling(strategy, existing.jobId, g);
        } else {
          setMsg($("sg-bt-msg"), err.message || t("sgBusy"), "err");
        }
      } else {
        setMsg($("sg-bt-msg"), (err && err.message) || String(err), "err");
      }
    } finally {
      if (g === gen) syncBtBtn(false);
    }
  }

  /// 1s 轮询; 离开视图 / 切策略由 deactivate 或代际令牌停表, 绝不在旧视图上改 DOM。
  function startPolling(strategy, jobId, g) {
    if (pollTimer) clearInterval(pollTimer);
    const tick = async () => {
      let info;
      try {
        info = await R.api("/api/backtest/" + encodeURIComponent(jobId));
      } catch (err) {
        if (g !== gen || !detailCtx || detailCtx.id !== strategy) return;
        // 404 = 作业过期被惰性清理; 其余网络抖动: 保留下一拍(运行态原样)。
        if (err && err.status === 404) {
          const j = jobs.get(strategy);
          if (j) {
            j.status = "error";
            j.error = t("sgBtExpired");
            renderJob(j);
          }
          if (pollTimer) clearInterval(pollTimer);
        }
        return;
      }
      if (g !== gen || !detailCtx || detailCtx.id !== strategy) return;
      const job = jobs.get(strategy);
      if (!job || job.jobId !== jobId) {
        if (pollTimer) clearInterval(pollTimer);
        return;
      }
      job.status = info.status;
      job.report = info.report || null;
      job.error = info.error || null;
      renderJob(job);
      if (info.status !== "running" && pollTimer) clearInterval(pollTimer);
    };
    pollTimer = setInterval(() => {
      tick().catch(() => {});
    }, POLL_MS);
  }

  /// 三态渲染(running 转圈 / done 报告 pre 保换行 / error 原因 + 重试)。
  function renderJob(job) {
    const box = $("sg-bt-result");
    box.innerHTML = "";
    if (job.status === "running") {
      const row = h("div", "sg-job running");
      row.appendChild(h("span", "sg-spinner"));
      row.appendChild(h("span", null, t("sgBtRunning")));
      box.appendChild(row);
      return;
    }
    if (job.status === "done") {
      box.appendChild(h("div", "sg-job-head sg-ok", t("sgBtDone")));
      const pre = h("pre", "sg-report");
      pre.textContent = job.report || "";
      box.appendChild(pre);
      return;
    }
    // error
    box.appendChild(h("div", "sg-job-head sg-err", t("sgBtError")));
    box.appendChild(h("div", "sg-msg err", job.error || ""));
    const retry = h("button", "sg-btn sg-btn-mini", t("sgBtRetry"));
    retry.type = "button";
    retry.addEventListener("click", () => rerunJob(job).catch((e) => setMsg($("sg-bt-msg"), e.message, "err")));
    box.appendChild(retry);
  }

  /// 用发起时的同一份参数重新发起(错误态一键重试)。
  async function rerunJob(job) {
    if (!detailCtx) return;
    const g = gen;
    syncBtBtn(true);
    const reply = await R.api("/api/backtest", { method: "POST", body: job.body });
    if (g !== gen) return;
    const next = {
      jobId: reply.job_id,
      status: "running",
      report: null,
      error: null,
      body: job.body,
    };
    jobs.set(detailCtx.id, next);
    renderJob(next);
    startPolling(detailCtx.id, next.jobId, g);
    syncBtBtn(false);
  }

  // ---------- 切语言: 动态文本就地刷新(静态标签由公共 applyLang 处理) ----------

  function refreshDynamic() {
    if (listData && !$("sg-listview").hidden) renderList(listData);
    if (detailCtx) {
      renderBadges(detailCtx.detail, detailCtx.source);
      $("sg-builtin-hint").textContent = t("sgBuiltinHint");
      updateDirty();
      syncSaveBtn(saveLocked ? "locked" : "idle");
      syncAiBtn(false);
      syncBtBtn(false);
      const job = jobs.get(detailCtx.id);
      if (job) renderJob(job);
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

  R.views.strategies = {
    /// param = 二级 hash 解码后的策略 id; 缺省 = 列表态。
    activate: (param) => {
      mount();
      const g = (gen += 1);
      R.applyLang(R.lang);
      $("sg-listview").hidden = !!param;
      $("sg-detail").hidden = !param;
      $("sg-modal-root").innerHTML = "";
      if (param) enterDetail(param, g);
      else enterList(g);
    },
    deactivate: () => {
      gen += 1; // 在途响应/轮询全部作废
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = 0;
      }
      $("sg-modal-root").innerHTML = "";
    },
  };
})();
