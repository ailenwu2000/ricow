// Ricow Web 前端「策略」视图 (032 US3, FR-015 ~ FR-021):
//   列表(市场 × 来源四组) / 复制内置 / 新建空白 / 详情(参数表单 + Lua 编辑器) /
//   保存(编译门禁, 运行中拒绝) / AI 改 Lua(草稿+撤销, need_keys 引导密钥页) / 回测异步作业(轮询三态)。
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
    sgUndeclared: "参数未声明",
    sgUndeclaredShort: "未声明清单",
    sgDupOf: "与内置重复",
    sgDeclHint: "这份策略没声明参数清单: 表单只画得出交易对, 必填参数不在表单里, 启动会因为「缺少必填参数」停机(0 成交)。到下方「策略清单」里声明参数即可补齐。",
    sgDupHint: "这份脚本与内置策略逐字相同: 再跑一份(尤其指向同一交易对)会重复下单。未声明清单时已禁止回测/启动; 把实例 TOML 的 type 直接配成内置策略, 或修改脚本 / 声明清单后即可运行。重复的是",
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
    sgNeedKeys: "AI 还没有可用密钥,先到密钥页添加一套后再试。",
    sgGoSettings: "去密钥页添加",
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
    sgAiNew: "+ AI 生成",
    sgAiNewTitle: "AI 生成新策略",
    sgTpl: "从模板新建",
    sgTplBlank: "空白骨架(不会交易, 需自己写 Lua)",
    sgTplLoading: "载入模板…",
    sgIdea: "策略思路",
    sgIdeaPh: "用几句话描述策略要做什么, 例如:价格跌破布林下轨时买入, 回到中轨卖出; 只在 RSI<30 时进场",
    sgConstraints: "风控与约束(可选)",
    sgConstraintsPh: "例如:单笔不超过总资金 10%; 亏损超 5% 止损; 每天最多交易 3 次",
    sgGen: "生成并保存",
    sgGenerating: "AI 生成中…(约 1–2 分钟)",
    sgMetrics: "关键指标",
    sgBenchShort: "基准",
    sgEqVsBench: "策略 vs 买入持有",
    sgNetPnl: "净盈亏",
    sgMaxDd: "最大回撤",
    sgWinRate: "胜率",
    sgTrades: "成交",
    sgFees: "手续费",
    sgSharpe: "夏普",
    sgRejected: "拒单",
    sgReportFull: "完整文本报告",
    // 039: 图表图例 + 平仓明细
    sgLegPrice: "标的价格",
    sgLegEquity: "策略权益",
    sgLegBench: "买入持有(建仓起)",
    sgLegDd: "回撤",
    sgClosedTitle: "平仓盈亏明细",
    sgClosedThTime: "时间",
    sgClosedThPnl: "已实现盈亏",
    sgClosedEmpty: "本次回测没有平仓事件(策略全程未平仓, 或从未建仓)。",
    sgClosedSum: "求和 = 已实现盈亏 ",
    sgClosedShownSum: "显示部分求和 = ",
    sgClosedMismatch: "与指标卡不一致(指标卡 = ",
    sgClosedTrunc: "明细仅保留最近 {n} 条(共 {m} 条)",
    sgSendToAi: "把结论带给 AI",
    sgHistory: "回测历史(本次会话)",
    sgHistoryTime: "时间",
    sgHistoryWindow: "窗口",
    sgHistoryEmpty: "还没有完成的回测。",
    sgZeroTradeHint:
      "本次回测零成交：常见原因是必填参数未填(如 start_price)、预热段不足或入场条件从未触发 —— 请展开下方「策略日志」查看策略自检与停机原因。",
    sgStrategyLogs: "策略日志",
    sgStrategyLogsHint: "策略运行期输出(含自检 / 停机原因 / 逐 tick 状态)。",
    sgStrategyLogsEmpty: "本策略未输出日志。",
    sgManifest: "参数清单(声明式)",
    sgManifestHint:
      "声明本策略有哪些参数(键名/类型/默认值/说明)。保存后上方「参数」表单、回测与寻优都会按它渲染 —— 只影响展示与表单, 不改动 Lua 代码。",
    sgMfName: "策略中文名",
    sgMfSummary: "一句话说明",
    sgMfSuitable: "适用场景",
    sgMfUnsuitable: "不适用场景",
    sgMfDesc: "长说明",
    sgMfKey: "键名",
    sgMfNameCol: "中文名",
    sgMfType: "类型",
    sgMfDefault: "默认值",
    sgMfRequired: "必填",
    sgMfDescCol: "说明",
    sgMfOptions: "可选值(逗号分隔)",
    sgMfAdd: "添加参数",
    sgMfSave: "保存清单",
    sgMfSaved: "清单已保存, 参数表单已更新。",
    sgMfEmpty: "还没有声明参数 —— 点「添加参数」开始。",
    sgMfDel: "删除",
    sgMfKeyPh: "如 grid_step_pct",
    sgMfNamePh: "如 网格间距倍数",
    sgMfDefaultPh: "留空 = 无默认",
    sgMfDescPh: "这个参数做什么",
    sgMfOptionsPh: "如 cash,equity",
    sgSweep: "参数寻优",
    sgSweepParam: "参数键",
    sgSweepValues: "档位取值(逗号分隔, 2–10 个)",
    sgSweepGo: "开始寻优",
    sgSweepRunning: "寻优运行中…",
    sgSweepValue: "档位",
    sgSweepEmpty: "先填写参数键与档位取值。",
    sgWarnings: "静态检查提示",
    sgApiRef: "ctx API 速查",
    sgJumpErr: "定位错误行",
    sgSavedWithWarn: "已保存(附静态检查提示)",
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
    sgUndeclared: "Params undeclared",
    sgUndeclaredShort: "No manifest",
    sgDupOf: "Duplicates built-in",
    sgDeclHint: "This strategy declares no parameter schema: the form renders only the pair, so required params are missing and the run halts with 0 trades. Declare them under “Strategy manifest” below.",
    sgDupHint: "This script is byte-identical to a built-in: running another copy (especially on the same pair) double-orders. While undeclared, backtest/start is blocked; point the instance TOML at the built-in type, or edit the script / declare a manifest. Duplicate of",
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
    sgNeedKeys: "No AI key is configured yet. Add one on the Keys page, then try again.",
    sgGoSettings: "Go to Keys",
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
    sgAiNew: "+ AI generate",
    sgAiNewTitle: "Generate strategy with AI",
    sgTpl: "New from template",
    sgTplBlank: "Blank skeleton (won't trade; write your own Lua)",
    sgTplLoading: "Loading template…",
    sgIdea: "Strategy idea",
    sgIdeaPh: "Describe what the strategy should do, e.g. buy below lower Bollinger band, sell at mid band; enter only when RSI<30",
    sgConstraints: "Risk & constraints (optional)",
    sgConstraintsPh: "e.g. max 10% of equity per order; stop loss at -5%; at most 3 trades per day",
    sgGen: "Generate & save",
    sgGenerating: "AI generating… (1–2 min)",
    sgMetrics: "Key metrics",
    sgBenchShort: "B&H",
    sgEqVsBench: "Strategy vs buy & hold",
    sgNetPnl: "Net PnL",
    sgMaxDd: "Max drawdown",
    sgWinRate: "Win rate",
    sgTrades: "Trades",
    sgFees: "Fees",
    sgSharpe: "Sharpe",
    sgRejected: "Rejected",
    sgReportFull: "Full text report",
    sgLegPrice: "Underlying price",
    sgLegEquity: "Strategy equity",
    sgLegBench: "Buy & hold (from entry)",
    sgLegDd: "Drawdown",
    sgClosedTitle: "Closed trades (realized PnL)",
    sgClosedThTime: "Time",
    sgClosedThPnl: "Realized PnL",
    sgClosedEmpty: "No closing event in this backtest (never closed, or never opened).",
    sgClosedSum: "Sum = realized PnL ",
    sgClosedShownSum: "Sum of shown = ",
    sgClosedMismatch: "differs from the metric card (card = ",
    sgClosedTrunc: "only the most recent {n} rows are kept ({m} in total)",
    sgSendToAi: "Send result to AI",
    sgHistory: "Backtest history (this session)",
    sgHistoryTime: "Time",
    sgHistoryWindow: "Window",
    sgHistoryEmpty: "No finished backtest yet.",
    sgZeroTradeHint:
      "No trades in this backtest: usually a missing required param (e.g. start_price), insufficient warmup, or entry conditions never met — expand Strategy logs below to see the strategy's own checks and halt reason.",
    sgStrategyLogs: "Strategy logs",
    sgStrategyLogsHint: "Strategy runtime output (self-checks / halt reason / per-tick state).",
    sgStrategyLogsEmpty: "This strategy produced no logs.",
    sgManifest: "Param manifest (declarative)",
    sgManifestHint:
      "Declare this strategy's params (key / type / default / desc). After saving, the Params form, backtest and sweep all render from it — it affects display and forms only, never your Lua code.",
    sgMfName: "Display name",
    sgMfSummary: "One-line summary",
    sgMfSuitable: "Suitable for",
    sgMfUnsuitable: "Not suitable for",
    sgMfDesc: "Long description",
    sgMfKey: "Key",
    sgMfNameCol: "Name",
    sgMfType: "Type",
    sgMfDefault: "Default",
    sgMfRequired: "Required",
    sgMfDescCol: "Description",
    sgMfOptions: "Options (comma separated)",
    sgMfAdd: "Add param",
    sgMfSave: "Save manifest",
    sgMfSaved: "Manifest saved; params form updated.",
    sgMfEmpty: "No params declared yet — click Add param to start.",
    sgMfDel: "Remove",
    sgMfKeyPh: "e.g. grid_step_pct",
    sgMfNamePh: "e.g. Grid step multiplier",
    sgMfDefaultPh: "empty = no default",
    sgMfDescPh: "What this param does",
    sgMfOptionsPh: "e.g. cash,equity",
    sgSweep: "Parameter sweep",
    sgSweepParam: "Param key",
    sgSweepValues: "Values (comma separated, 2–10)",
    sgSweepGo: "Run sweep",
    sgSweepRunning: "Sweep running…",
    sgSweepValue: "Value",
    sgSweepEmpty: "Fill in the param key and values first.",
    sgWarnings: "Static check hints",
    sgApiRef: "ctx API reference",
    sgJumpErr: "Go to error line",
    sgSavedWithWarn: "Saved (with static check hints)",
  });

  // 常量(与后端白名单同口径)。
  const INTERVALS = ["1m", "5m", "15m", "1h", "4h", "1d"];
  const DEFAULT_INTERVAL = "1h";
  const DEFAULT_PAIR = "BTCUSDT";
  const NAME_RE = /^[A-Za-z0-9_-]{1,24}$/;
  const POLL_MS = 1000;
  // 参数表单里**不该再画一行**的保留键 = 后端 `strategy_io::RESERVED_PARAM_KEYS`
  // (`script`/`script_path`, 清单里禁止声明) **加上** `pair`:
  // - `pair` 后端**允许**在清单里声明(内置两份都声明了, 用于给"交易对"这个字段配中文名与必填),
  //   但表单里已由 `#sg-pair` 独立成字段, 再按清单画一行就是被顶层值盖掉的死控件(2026-10-05 修)。
  // - `script`/`script_path` 指脚本本身, 从来不是用户可填参数。
  const RESERVED_PARAM_KEYS = ["pair", "script", "script_path"];

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

  /// ctx API 速查(P1-4): 浓缩自 specs/lua-api.md, 静态内容无插值 —— 服务端文本绝不走这里。
  const API_REF_HTML =
    '<table class="sg-apiref-table"><tbody>' +
    "<tr><td>行情</td><td><code>ctx:price(pair)</code> 当前价(本 bar 开盘, 无前视)<br>" +
    "<code>ctx:best_bid / best_ask(pair)</code> 盘口<br>" +
    "<code>ctx:klines(pair)</code> 已收盘 K 线数组(尾窗化)</td></tr>" +
    "<tr><td>持仓</td><td><code>ctx:position_side / position_size / position_entry(pair)</code><br>" +
    "<code>ctx:balance(asset)</code> <code>ctx:equity()</code> <code>ctx:net_pnl()</code></td></tr>" +
    "<tr><td>参数</td><td>读参数用 <code>num(ctx,key,缺省)</code> / <code>cfg_str(ctx,key,缺省)</code> 辅助;" +
    "直接 <code>ctx:config_f64(...) or 缺省</code> 是陷阱(未配置读到 0)</td></tr>" +
    "<tr><td>指标</td><td><code>ctx:ema/sma/rsi/atr/boll/macd/adx/stoch(pair, n)</code> — " +
    "基于已收盘 K 线, 数据不足返回 nil, 用 <code>if v then</code> 判断<br>" +
    "高周期: 先 <code>ctx:need_klines(\"aux\", tf, bars)</code> 声明, 再 " +
    "<code>ctx:atr_tf / ema_tf / close_tf(pair, tf, ...)</code></td></tr>" +
    "<tr><td>下单</td><td>on_tick 返回订单数组: " +
    "<code>{ pair=, side=\"buy\"|\"sell\", size=, price=, order_type=\"limit\"|\"market\", reduce_only= }</code>" +
    "(price 省略 = 市价单)</td></tr>" +
    "<tr><td>成交</td><td>on_fill(ctx, fill): 字段是 <code>fill_price / fill_size / fee / side</code>" +
    "(不是 price/size)</td></tr>" +
    "<tr><td>注意</td><td>沙箱禁 os/io/require/pcall; 模块级 local 不跨 tick 保留, 状态用全局变量;" +
    "数组下标从 1 开始</td></tr>" +
    "</tbody></table>";

  /// 图表抽稀上限(前端兜底, 与后端 CHART_MAX_POINTS 呼应)。
  const CHART_MAX_POINTS = 2000;

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
  // 寻优作业(P2-7): 同一台账的另一类条目, key = strategyId + "#sweep"。
  // 回测历史(P1-6, 仅本次会话内存; 不写浏览器持久存储)。
  const jobHistory = [];
  // 当前图表实例(重渲染/离开前 remove, 同 markets.js 的图表生命周期)。
  let btChart = null;

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
      '<span class="sg-head-actions"><button id="sg-ai-new" class="sg-btn" type="button"' +
      bi("sgAiNew") + ">" + R.TEXT.zh.sgAiNew + "</button>" +
      '<button id="sg-new" class="sg-btn sg-btn-primary" type="button"' + bi("sgNew") + ">" +
      R.TEXT.zh.sgNew + "</button></span></div>" +
      '<div id="sg-list-msg" class="sg-msg" aria-live="polite"></div>' +
      '<div id="sg-list-body" class="sg-groups"></div></div>' +
      // ---- 详情态 ----
      '<div id="sg-detail" class="sg-detail" hidden>' +
      '<div class="sg-crumbs"><a href="javascript:void(0)" id="sg-back"' + bi("sgBack") + ">" +
      R.TEXT.zh.sgBack + "</a></div>" +
      '<div class="sg-detail-head"><h2 id="sg-d-name" class="sg-d-name"></h2>' +
      '<span id="sg-d-badges" class="sg-badges"></span></div>' +
      '<div id="sg-builtin-hint" class="sg-hint" hidden></div>' +
      // 诚实性提示(2026-10-05): 未声明参数清单 / 与内置脚本逐字相同 —— 文案由 renderDeclHint 填。
      '<div id="sg-decl-hint" class="sg-hint" hidden></div>' +
      '<div id="sg-d-loadmsg"></div>' +
      '<div class="sg-detail-grid">' +
      // 参数
      '<section class="sg-card"><div class="sg-card-head"><span' + bi("sgParams") + ">" +
      R.TEXT.zh.sgParams + "</span></div><div id='sg-params' class='sg-card-body'></div></section>" +
      // 参数清单编辑(P2-8): 仅用户策略可见(内置清单随代码发布, 由 renderDetail 决定显隐)。
      '<section class="sg-card" id="sg-manifest-card" hidden>' +
      '<details class="sg-advanced sg-manifest"><summary' + bi("sgManifest") + ">" +
      R.TEXT.zh.sgManifest + "</summary>" +
      '<div class="sg-hint"' + bi("sgManifestHint") + ">" + R.TEXT.zh.sgManifestHint + "</div>" +
      "<label class='sg-field'><span" + bi("sgMfName") + ">" + R.TEXT.zh.sgMfName + "</span>" +
      '<input id="sg-mf-name" class="sg-input" type="text" autocomplete="off" /></label>' +
      "<label class='sg-field'><span" + bi("sgMfSummary") + ">" + R.TEXT.zh.sgMfSummary + "</span>" +
      '<input id="sg-mf-summary" class="sg-input" type="text" autocomplete="off" /></label>' +
      "<label class='sg-field'><span" + bi("sgMfSuitable") + ">" + R.TEXT.zh.sgMfSuitable +
      "</span><input id='sg-mf-suitable' class='sg-input' type='text' autocomplete='off' /></label>" +
      "<label class='sg-field'><span" + bi("sgMfUnsuitable") + ">" + R.TEXT.zh.sgMfUnsuitable +
      "</span><input id='sg-mf-unsuitable' class='sg-input' type='text' autocomplete='off' /></label>" +
      "<label class='sg-field'><span" + bi("sgMfDesc") + ">" + R.TEXT.zh.sgMfDesc +
      "</span><textarea id='sg-mf-desc' class='sg-input' rows='2'></textarea></label>" +
      '<div class="sg-mf-head"><span' + bi("sgMfKey") + ">" + R.TEXT.zh.sgMfKey +
      "</span><span" + bi("sgMfNameCol") + ">" + R.TEXT.zh.sgMfNameCol +
      "</span><span" + bi("sgMfType") + ">" + R.TEXT.zh.sgMfType +
      "</span><span" + bi("sgMfDefault") + ">" + R.TEXT.zh.sgMfDefault + "</span>" +
      "<span" + bi("sgMfRequired") + ">" + R.TEXT.zh.sgMfRequired + "</span><span></span></div>" +
      '<div id="sg-mf-params" class="sg-mf-params"></div>' +
      '<div class="sg-row-right"><button id="sg-mf-add" class="sg-btn sg-btn-mini" type="button"' +
      bi("sgMfAdd") + ">" + R.TEXT.zh.sgMfAdd + "</button>" +
      '<button id="sg-mf-save" class="sg-btn sg-btn-mini sg-btn-primary" type="button"' +
      bi("sgMfSave") + ">" + R.TEXT.zh.sgMfSave + "</button></div>" +
      '<div id="sg-mf-msg" class="sg-msg" aria-live="polite"></div></details></section>' +
      // Lua 编辑器
      '<section class="sg-card sg-code-card"><div class="sg-card-head"><span' + bi("sgCode") + ">" +
      R.TEXT.zh.sgCode + "</span>" +
      '<span id="sg-dirty" class="sg-dirty" hidden><span id="sg-dirty-text"></span>' +
      '<button id="sg-undo" class="sg-btn sg-btn-mini" type="button"' + bi("sgUndo") + ">" +
      R.TEXT.zh.sgUndo + "</button>" +
      '<button id="sg-save-top" class="sg-btn sg-btn-mini sg-btn-primary" type="button"' +
      bi("sgSave") + ">" + R.TEXT.zh.sgSave + "</button></span></div>" +
      '<div id="sg-savemsg" class="sg-msg" aria-live="polite"></div>' +
      '<div id="sg-warnlist" class="sg-warnlist" hidden></div>' +
      '<details class="sg-advanced sg-apiref"><summary' + bi("sgApiRef") + ">" +
      R.TEXT.zh.sgApiRef + "</summary>" + API_REF_HTML + "</details>" +
      '<div class="sg-editor-wrap"><pre id="sg-hl" class="sg-hl" aria-hidden="true"></pre>' +
      '<textarea id="sg-code" class="sg-code" spellcheck="false" autocomplete="off"' +
      ' wrap="off"></textarea></div>' +
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
      '<div id="sg-bt-result"></div>' +
      // 参数寻优(P2-7)与历史对比(P1-6)。
      '<details class="sg-advanced sg-sweep-box" id="sg-sweep-box"><summary' + bi("sgSweep") + ">" +
      R.TEXT.zh.sgSweep + "</summary><div class='sg-bt-grid'>" +
      "<label><span" + bi("sgSweepParam") + ">" + R.TEXT.zh.sgSweepParam + "</span>" +
      '<select id="sg-sw-param" class="sg-input"></select></label>' +
      "<label><span" + bi("sgSweepValues") + ">" + R.TEXT.zh.sgSweepValues + "</span>" +
      '<input id="sg-sw-values" class="sg-input" type="text" autocomplete="off" /></label>' +
      "</div>" +
      '<div class="sg-row-right"><button id="sg-sw-go" class="sg-btn" type="button"' +
      bi("sgSweepGo") + ">" + R.TEXT.zh.sgSweepGo + "</button></div>" +
      '<div id="sg-sw-msg" class="sg-msg" aria-live="polite"></div>' +
      '<div id="sg-sw-result"></div></details>' +
      '<div id="sg-bt-history"></div></div></section>' +
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
    $("sg-ai-new").addEventListener("click", openAiGenModal);
    $("sg-back").addEventListener("click", () => R.navigate("strategies"));
    $("sg-code").addEventListener("keydown", onCodeKeydown);
    $("sg-code").addEventListener("input", () => {
      updateDirty();
      refreshHighlight();
    });
    // 高亮层与 textarea 滚动同步(经典 overlay 双层编辑器)。
    $("sg-code").addEventListener("scroll", () => {
      const hl = $("sg-hl");
      hl.scrollTop = $("sg-code").scrollTop;
      hl.scrollLeft = $("sg-code").scrollLeft;
    });
    $("sg-undo").addEventListener("click", onUndo);
    $("sg-save").addEventListener("click", onSave);
    $("sg-save-top").addEventListener("click", onSave);
    $("sg-ai-go").addEventListener("click", onAiEdit);
    $("sg-bt-go").addEventListener("click", onBacktest);
    $("sg-sw-go").addEventListener("click", onSweep);
    $("sg-mf-add").addEventListener("click", () => {
      addManifestRow(null);
      setMsg($("sg-mf-msg"), "", "");
    });
    $("sg-mf-save").addEventListener("click", onSaveManifest);
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
    // 诚实性徽章(2026-10-05): 未声明参数清单 / 与内置脚本逐字相同 —— 列表里就能看出来,
    // 不用点进去才发现参数表单是空的。
    const flags = h("span", "sg-badges");
    if (s.declared === false) {
      flags.appendChild(h("span", "sg-badge sg-badge-warn", t("sgUndeclaredShort")));
    }
    if (s.duplicate_of) {
      const dup = h("span", "sg-badge sg-badge-warn", t("sgDupOf"));
      dup.title = s.duplicate_of; // 服务端数据只走 textContent / 属性赋值, 绝不拼 HTML
      flags.appendChild(dup);
    }
    if (flags.childNodes.length) main.appendChild(flags);
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

  // ---------- 新建(模板向导, P0-1) ----------

  /// 模板来源 = 内置策略(自带 Lua + 参数清单) + 空白骨架。选模板后拉取
  /// 清单(参数 schema)与源码, 预填默认参数 —— 用户 0 行代码也能得到可跑的起点。
  function openNewModal() {
    const { card, close } = openModal(t("sgModalNew"));
    const form = h("div", "sg-modal-form");

    // 模板选择: 内置策略 + 空白骨架。
    const tplLabel = h("label", "sg-field");
    tplLabel.appendChild(h("span", null, t("sgTpl")));
    const tplSel = document.createElement("select");
    tplSel.className = "sg-input";
    const blankOpt = document.createElement("option");
    blankOpt.value = "";
    blankOpt.textContent = t("sgTplBlank");
    tplSel.appendChild(blankOpt);
    const builtins = (listData || []).filter((s) => s.source === "builtin");
    for (const s of builtins) {
      const opt = document.createElement("option");
      opt.value = s.id;
      opt.textContent = s.name + " (" + s.market + " · " + s.id + ")";
      tplSel.appendChild(opt);
    }
    tplLabel.appendChild(tplSel);
    form.appendChild(tplLabel);

    // 名称 / 市场 / 交易对 / 参数区(随模板变化)。
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

    // 模板参数表单(选择模板后渲染; mparams 为弹窗局部注册表)。
    const paramsBox = h("div", "sg-modal-params");
    let mparams = [];
    form.appendChild(paramsBox);

    const errLine = h("div", "sg-msg err");
    form.appendChild(errLine);
    card.appendChild(form);
    const { cancel, primary } = modalActions(card, "sgCreate");
    cancel.addEventListener("click", close);

    // 模板状态: 选中内置后缓存 {market, lua, detail}。
    let tpl = null;
    let tplReq = 0;

    async function onTplChange() {
      const id = tplSel.value;
      tpl = null;
      mparams = [];
      paramsBox.innerHTML = "";
      marketSel.disabled = false;
      if (!id) return; // 空白骨架
      const req = ++tplReq;
      setMsg(errLine, t("sgTplLoading"), "");
      try {
        const [detail, source] = await Promise.all([
          R.api("/api/strategies/" + encodeURIComponent(id)),
          R.api("/api/strategies/" + encodeURIComponent(id) + "/source"),
        ]);
        if (req !== tplReq) return;
        tpl = { market: detail.market, lua: source.lua, detail: detail };
        marketSel.value = detail.market;
        marketSel.disabled = true;
        // 参数表单: 清单 schema + 默认值预填(必填无默认的留空由用户填)。
        for (const p of detail.params || []) {
          if (RESERVED_PARAM_KEYS.includes(p.key)) continue; // 交易对/脚本键独立成字段
          const built = p.ty === "string"
            ? stringRow(p, p.default !== undefined ? p.default : undefined)
            : paramRow(p, p.default !== undefined ? p.default : undefined);
          paramsBox.appendChild(built.label);
          mparams.push({ key: p.key, ty: p.ty, required: !!p.required, input: built.input });
        }
        setMsg(errLine, "", "");
        if (!nameInput.value) nameInput.value = suggestName(id);
      } catch (err) {
        if (req !== tplReq) return;
        setMsg(errLine, (err && err.message) || String(err), "err");
      }
    }
    tplSel.addEventListener("change", () => onTplChange().catch(strategyErr));

    async function submit() {
      const name = nameInput.value.trim();
      const pair = pairInput.value.trim();
      if (!NAME_RE.test(name)) {
        setMsg(errLine, t("sgNameInvalid"), "err");
        return;
      }
      if (!pair) {
        setMsg(errLine, t("sgPairRequired"), "err");
        return;
      }
      // 收集模板参数(缺省跳过 —— Lua 内 fallback 与清单 default 兜底)。
      const params = {};
      for (const p of mparams) {
        const v = readParam(p);
        if (v !== undefined) params[p.key] = v;
      }
      primary.disabled = true;
      primary.textContent = t("sgCreating");
      try {
        await R.api("/api/strategies", {
          method: "POST",
          body: {
            name: name,
            market: tpl ? tpl.market : marketSel.value,
            pair: pair,
            code: tpl ? tpl.lua : BLANK_LUA,
            params: params,
            overwrite: false,
          },
        });
        close();
        openDetail(name);
      } catch (err) {
        // reserved/prefix/exists/compile/invalid_name 全部由服务端中文原文回填, 不关弹窗。
        setMsg(errLine, (err && err.message) || String(err), "err");
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

  // ---------- AI 生成新策略 (P0-3) ----------

  /// 结构化意图(思路/市场/标的/周期/风控) → /api/strategies/ai-generate(过编译门禁,
  /// 不落盘) → POST /api/strategies 落盘 → 打开详情继续回测调参。
  function openAiGenModal() {
    const { card, close } = openModal(t("sgAiNewTitle"));
    const form = h("div", "sg-modal-form");

    function field(labelKey, ctl) {
      const label = h("label", "sg-field");
      label.appendChild(h("span", null, t(labelKey)));
      label.appendChild(ctl);
      form.appendChild(label);
      return label;
    }

    const nameInput = document.createElement("input");
    nameInput.className = "sg-input";
    nameInput.type = "text";
    nameInput.autocomplete = "off";
    nameInput.spellcheck = false;
    field("sgName", nameInput);
    form.appendChild(h("div", "sg-field-hint", t("sgNameHint")));

    const row = h("div", "sg-gen-row");
    const marketSel = document.createElement("select");
    marketSel.className = "sg-input";
    for (const [value, label] of [["spot", t("spot")], ["futures", t("futures")]]) {
      const opt = document.createElement("option");
      opt.value = value;
      opt.textContent = label;
      marketSel.appendChild(opt);
    }
    const pairInput = document.createElement("input");
    pairInput.className = "sg-input";
    pairInput.type = "text";
    pairInput.value = DEFAULT_PAIR;
    pairInput.autocomplete = "off";
    const ivSel = document.createElement("select");
    ivSel.className = "sg-input";
    for (const iv of INTERVALS) {
      const opt = document.createElement("option");
      opt.value = iv;
      opt.textContent = iv;
      if (iv === DEFAULT_INTERVAL) opt.selected = true;
      ivSel.appendChild(opt);
    }
    row.appendChild(wrapField(t("sgMarket"), marketSel));
    row.appendChild(wrapField(t("sgPair"), pairInput));
    row.appendChild(wrapField(t("sgInterval"), ivSel));
    form.appendChild(row);

    const idea = document.createElement("textarea");
    idea.className = "sg-ai-instr";
    idea.rows = 4;
    idea.spellcheck = false;
    idea.placeholder = R.TEXT.zh.sgIdeaPh;
    idea.setAttribute("data-zh-placeholder", R.TEXT.zh.sgIdeaPh);
    idea.setAttribute("data-en-placeholder", R.TEXT.en.sgIdeaPh);
    form.appendChild(wrapField(t("sgIdea"), idea));

    const constraints = document.createElement("textarea");
    constraints.className = "sg-ai-instr";
    constraints.rows = 2;
    constraints.spellcheck = false;
    constraints.setAttribute("data-zh-placeholder", R.TEXT.zh.sgConstraintsPh);
    constraints.setAttribute("data-en-placeholder", R.TEXT.en.sgConstraintsPh);
    form.appendChild(wrapField(t("sgConstraints"), constraints));

    const errLine = h("div", "sg-msg err");
    form.appendChild(errLine);
    card.appendChild(form);
    const { cancel, primary } = modalActions(card, "sgGen");
    cancel.addEventListener("click", close);

    // 弹窗内的小字段包装(不进 form, 供 row 布局)。
    function wrapField(labelText, ctl) {
      const label = h("label", "sg-field");
      label.appendChild(h("span", null, labelText));
      label.appendChild(ctl);
      return label;
    }

    async function submit() {
      const name = nameInput.value.trim();
      if (!NAME_RE.test(name)) {
        setMsg(errLine, t("sgNameInvalid"), "err");
        return;
      }
      if (!pairInput.value.trim()) {
        setMsg(errLine, t("sgPairRequired"), "err");
        return;
      }
      if (!idea.value.trim()) {
        setMsg(errLine, t("sgIdeaPh"), "err");
        return;
      }
      primary.disabled = true;
      primary.textContent = t("sgGenerating");
      setMsg(errLine, "", "");
      try {
        // ① AI 生成(过编译门禁, 未落盘)。
        const gen = await R.api("/api/strategies/ai-generate", {
          method: "POST",
          body: {
            name: name,
            market: marketSel.value,
            pair: pairInput.value.trim(),
            interval: ivSel.value,
            idea: idea.value.trim(),
            constraints: constraints.value.trim(),
          },
        });
        // ② 落盘(与手动保存同一端点; params 空 = 用 Lua 内 fallback)。
        await R.api("/api/strategies", {
          method: "POST",
          body: {
            name: name,
            market: marketSel.value,
            pair: pairInput.value.trim(),
            code: gen.code,
            params: {},
            overwrite: false,
          },
        });
        close();
        openDetail(name);
      } catch (err) {
        if (err && err.status === 403 && err.code === "need_keys") {
          showNeedKeysInline(errLine, err.message || t("sgNeedKeys"));
        } else {
          setMsg(errLine, (err && err.message) || String(err), "err");
        }
        primary.disabled = false;
        primary.textContent = t("sgGen");
      }
    }
    primary.addEventListener("click", () => submit().catch(strategyErr));
    nameInput.focus();
  }

  /// 弹窗内的 need_keys 引导(与详情页 showNeedKeys 同语义, 容器不同)。
  function showNeedKeysInline(el, message) {
    el.innerHTML = "";
    el.className = "sg-msg err";
    el.appendChild(h("span", null, message + " "));
    const go = h("button", "sg-btn sg-btn-mini", t("sgGoSettings"));
    go.type = "button";
    go.addEventListener("click", () => R.navigate("keys"));
    el.appendChild(go);
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
    renderDeclHint(detail);

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
    // 参数清单编辑器(P2-8): 用户策略可声明/维护参数 schema; 内置策略该卡隐藏。
    renderManifestEditor(detail);

    // savedCode 必须取 textarea getter 规范化后的值(CRLF→LF): 直接存服务端原文会让
    // CRLF 策略(Windows 全部)在 updateDirty 里恒判 dirty(「有未保存的修改」常亮)。
    $("sg-code").value = source.lua || "";
    savedCode = $("sg-code").value;
    aiDraft = false;
    updateDirty();
    setMsg($("sg-savemsg"), "", "");

    // 回测表单 pair 跟随当前实例, 周期/天数保留用户上次输入。
    if (!$("sg-bt-pair").value) $("sg-bt-pair").value = detail.pair || DEFAULT_PAIR;

    // 参数寻优下拉(P2-7): 参数键来自清单注册表 + 实例当前值(去重)。
    fillSweepParams();
    // 编辑器高亮层(P1-4)随内容初始化。
    refreshHighlight();
    $("sg-warnlist").hidden = true;

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
    // 诚实性徽章(2026-10-05)。
    if (detail.declared === false) {
      box.appendChild(h("span", "sg-badge sg-badge-warn", t("sgUndeclared")));
    }
    if (detail.duplicate_of) {
      const dup = h("span", "sg-badge sg-badge-warn", t("sgDupOf"));
      dup.title = detail.duplicate_of;
      box.appendChild(dup);
    }
  }

  /// 诚实性提示条: 未声明参数清单 / 与内置脚本逐字相同 —— 把后果(缺必填参数停机、
  /// 重复下单)和改法一并说清, 用户不用自己去撞(2026-10-05)。
  function renderDeclHint(detail) {
    const box = $("sg-decl-hint");
    const parts = [];
    if (detail.declared === false) parts.push(t("sgDeclHint"));
    if (detail.duplicate_of) parts.push(t("sgDupHint") + " " + detail.duplicate_of + "。");
    box.textContent = parts.join(" ");
    box.hidden = parts.length === 0;
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
      // 保留键跳过: `pair` 上面已独立成 `#sg-pair` 字段, `script`/`script_path` 指脚本本身 ——
      // 再来一行就是被顶层值盖掉的死控件(2026-10-05 修)。
      if (RESERVED_PARAM_KEYS.includes(p.key)) continue;
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

  // ---------- 参数清单 (manifest) 编辑 (P2-8) ----------
  //
  // 用户自写策略默认没有清单 → 参数表单空缺。这里让用户为自己的策略声明参数 schema,
  // 落盘 strategies/{market}/{id}.toml (与内置清单同构同目录), catalog 扫描后参数表单随即点亮。

  /// 清单编辑里的当前参数行(保存时逐行收集; 不含空行)。
  let manifestRows = [];

  /// 小号文本输入(清单行内的紧凑字段)。
  function mfInput(cls, value, placeholder) {
    const el = document.createElement("input");
    el.className = "sg-input " + cls;
    el.type = "text";
    el.autocomplete = "off";
    el.spellcheck = false;
    if (value) el.value = value;
    if (placeholder) el.placeholder = placeholder;
    return el;
  }

  /// 渲染清单编辑器(仅用户策略): 顶层元数据 + 参数行。内置策略整卡隐藏。
  function renderManifestEditor(detail) {
    const card = $("sg-manifest-card");
    if (!card) return;
    if (!detailCtx || detailCtx.builtin) {
      card.hidden = true;
      return;
    }
    card.hidden = false;
    $("sg-mf-name").value = detail.name || "";
    $("sg-mf-summary").value = detail.summary || "";
    $("sg-mf-suitable").value = detail.suitable || "";
    $("sg-mf-unsuitable").value = detail.unsuitable || "";
    $("sg-mf-desc").value = detail.description || "";
    const box = $("sg-mf-params");
    box.innerHTML = "";
    manifestRows = [];
    for (const p of detail.params || []) addManifestRow(p);
    setMsg($("sg-mf-msg"), manifestRows.length ? "" : t("sgMfEmpty"), "");
  }

  /// 追加一个参数行(p 为 null 时空行)。type 为 enum 时展开"可选值"输入。
  function addManifestRow(p) {
    const spec = p || {};
    const row = h("div", "sg-mf-row");

    const key = mfInput("sg-mf-k", spec.key || "", t("sgMfKeyPh"));
    const name = mfInput("sg-mf-n", spec.name || "", t("sgMfNamePh"));
    const ty = document.createElement("select");
    ty.className = "sg-input sg-mf-ty";
    for (const v of ["f64", "i64", "string", "bool", "enum"]) {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = v;
      ty.appendChild(o);
    }
    ty.value = spec.ty || "f64";
    const defVal =
      spec.default === undefined || spec.default === null ? "" : String(spec.default);
    const def = mfInput("sg-mf-def", defVal, t("sgMfDefaultPh"));

    const reqWrap = h("label", "sg-mf-reqwrap");
    const req = document.createElement("input");
    req.type = "checkbox";
    req.checked = !!spec.required;
    reqWrap.appendChild(req);
    reqWrap.appendChild(document.createTextNode(t("sgMfRequired")));

    const del = h("button", "sg-btn sg-btn-mini sg-mf-del", t("sgMfDel"));
    del.type = "button";

    const line1 = h("div", "sg-mf-line");
    line1.append(key, name, ty, def, reqWrap, del);

    const desc = mfInput("sg-mf-d", spec.desc || "", t("sgMfDescPh"));
    const opts = mfInput("sg-mf-o", (spec.options || []).join(","), t("sgMfOptionsPh"));
    const line2 = h("div", "sg-mf-line2");
    line2.append(desc, opts);

    const syncOpts = () => {
      opts.style.display = ty.value === "enum" ? "" : "none";
    };
    ty.addEventListener("change", syncOpts);
    syncOpts();

    del.addEventListener("click", () => {
      manifestRows = manifestRows.filter((r) => r.row !== row);
      row.remove();
    });

    row.append(line1, line2);
    manifestRows.push({ row: row, key: key, name: name, ty: ty, desc: desc, def: def, req: req, opts: opts });
    $("sg-mf-params").appendChild(row);
    return row;
  }

  /// 收集清单表单(空行跳过; 默认值按类型转成 JSON 数字/布尔/字符串)。
  function collectManifest() {
    const params = [];
    for (const r of manifestRows) {
      const key = r.key.value.trim();
      const name = r.name.value.trim();
      if (!key && !name) continue;
      const ty = r.ty.value;
      const p = { key: key, name: name, type: ty, desc: r.desc.value.trim(), required: r.req.checked };
      const raw = r.def.value.trim();
      if (raw !== "") {
        if (ty === "f64") p.default = Number(raw);
        else if (ty === "i64") p.default = Number(raw);
        else if (ty === "bool") p.default = raw === "true" || raw === "1";
        else p.default = raw;
      }
      if (ty === "enum") {
        p.options = r.opts.value
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s);
      }
      params.push(p);
    }
    return {
      name: $("sg-mf-name").value.trim(),
      summary: $("sg-mf-summary").value.trim(),
      description: $("sg-mf-desc").value.trim(),
      suitable: $("sg-mf-suitable").value.trim(),
      unsuitable: $("sg-mf-unsuitable").value.trim(),
      params: params,
    };
  }

  /// 保存清单 → 重取详情 → 参数表单/寻优下拉立即按新 schema 重建(不动编辑器里的代码)。
  async function onSaveManifest() {
    if (!detailCtx) return;
    const g = gen;
    const btn = $("sg-mf-save");
    btn.disabled = true;
    try {
      await R.api("/api/strategies/" + encodeURIComponent(detailCtx.id) + "/manifest", {
        method: "POST",
        body: collectManifest(),
      });
      if (g !== gen) return;
      setMsg($("sg-mf-msg"), t("sgMfSaved"), "ok");
      await refreshSchema(g);
    } catch (err) {
      if (g !== gen) return;
      setMsg($("sg-mf-msg"), (err && err.message) || String(err), "err");
    } finally {
      if (g === gen) btn.disabled = false;
    }
  }

  /// 重取详情并只重建"清单相关"的视图(参数表单 + 清单编辑器 + 寻优参数下拉),
  /// 刻意不碰代码编辑器(避免丢掉未保存的代码草稿)。
  async function refreshSchema(g) {
    const id = detailCtx && detailCtx.id;
    if (!id) return;
    const detail = await R.api("/api/strategies/" + encodeURIComponent(id));
    if (g !== gen || !detailCtx || detailCtx.id !== id) return;
    detailCtx.detail = detail;
    renderParams(detail);
    $("sg-pair").value = detail.pair || $("sg-pair").value || DEFAULT_PAIR;
    renderManifestEditor(detail);
    fillSweepParams();
  }

  /// 寻优参数下拉: 键来自当前参数表单(清单 + 实例值), 去重。
  function fillSweepParams() {
    const sel = $("sg-sw-param");
    if (!sel) return;
    const prev = sel.value;
    sel.innerHTML = "";
    const seen = new Set();
    for (const p of paramInputs) {
      if (seen.has(p.key)) continue;
      seen.add(p.key);
      const o = document.createElement("option");
      o.value = p.key;
      o.textContent = p.key;
      sel.appendChild(o);
    }
    if (prev && seen.has(prev)) sel.value = prev;
  }

  // ---------- 编辑器脏状态 / 高亮 / Tab / 撤销 (P1-4) ----------

  /// Lua 语法高亮(单遍正则 tokenizer): 注释/字符串/数字/关键字/ctx·exec API。
  /// 内容经 escape 后才进 innerHTML —— 用户与 AI 的代码都不可信, 只以纯文本形态着色。
  function highlightLua(src) {
    const esc = (s) =>
      s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const re =
      /(--\[\[[\s\S]*?\]\]|--[^\n]*)|("(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*')|\b(0x[0-9a-fA-F]+|\d+\.?\d*)\b|\b(function|end|if|then|elseif|else|for|while|do|return|local|and|or|not|nil|true|false|break|repeat|until|in)\b|(ctx:\w+|exec\.\w+)/g;
    let out = "";
    let last = 0;
    let m;
    while ((m = re.exec(src))) {
      out += esc(src.slice(last, m.index));
      const full = m[0];
      if (m[1]) out += '<span class="hl-com">' + esc(full) + "</span>";
      else if (m[2]) out += '<span class="hl-str">' + esc(full) + "</span>";
      else if (m[3]) out += '<span class="hl-num">' + esc(full) + "</span>";
      else if (m[4]) out += '<span class="hl-kw">' + esc(full) + "</span>";
      else out += '<span class="hl-api">' + esc(full) + "</span>";
      last = m.index + full.length;
    }
    out += esc(src.slice(last));
    // 末尾补一个换行: pre 与 textarea 的滚动高度对齐(软换行场景的宽高一致性)。
    return out + "\n";
  }

  /// 把 textarea 当前内容同步进高亮层(内容与滚动双同步)。
  function refreshHighlight() {
    const ta = $("sg-code");
    const hl = $("sg-hl");
    if (!ta || !hl) return;
    hl.innerHTML = highlightLua(ta.value);
    hl.scrollTop = ta.scrollTop;
    hl.scrollLeft = ta.scrollLeft;
  }

  /// 编译错误行定位: 把光标移到该行并选中, 编辑器滚到可见。
  function jumpToErrorLine(line) {
    if (typeof line !== "number" || line < 1) return;
    const ta = $("sg-code");
    const lines = ta.value.split("\n");
    if (line > lines.length) return;
    let start = 0;
    for (let i = 0; i < line - 1; i++) start += lines[i].length + 1;
    ta.focus();
    ta.setSelectionRange(start, start + lines[line - 1].length);
    // 粗略滚动到该行(行高一致, 由 CSS 保证)。
    const lh = parseFloat(getComputedStyle(ta).lineHeight) || 20;
    ta.scrollTop = Math.max(0, (line - 5) * lh);
    refreshHighlight();
  }

  /// 静态检查提示(P1-5): 黄色提示列表, 只提示不拦截。
  function renderWarnings(list) {
    const box = $("sg-warnlist");
    box.innerHTML = "";
    if (!list || !list.length) {
      box.hidden = true;
      return;
    }
    box.appendChild(h("span", "sg-warnlist-title", t("sgWarnings")));
    const ul = h("ul", "sg-warnlist-ul");
    for (const w of list) ul.appendChild(h("li", null, w));
    box.appendChild(ul);
    box.hidden = false;
  }

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
    renderWarnings([]);
    updateDirty();
    refreshHighlight();
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
      const reply = await R.api("/api/strategies", {
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
      refreshHighlight();
      setMsg(
        $("sg-savemsg"),
        reply && reply.warnings && reply.warnings.length ? t("sgSavedWithWarn") : t("sgSaved"),
        "ok"
      );
      renderWarnings(reply && reply.warnings);
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
        jumpToErrorLine(err.line);
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
      refreshHighlight();
      setMsg($("sg-ai-msg"), t("sgAiOk"), "ok");
      renderWarnings(reply.warnings);
    } catch (err) {
      if (g !== gen) return;
      if (err && err.status === 403 && err.code === "need_keys") {
        showNeedKeys(err.message || t("sgNeedKeys"));
      } else if (err && err.code === "compile") {
        // 编译没过: 服务端没给可⽤代码, 编辑器内容**不替换**。
        setMsg($("sg-ai-msg"), t("sgAiCompilePrefix") + " " + compileText(err), "err");
        jumpToErrorLine(err.line);
      } else {
        setMsg($("sg-ai-msg"), (err && err.message) || String(err), "err");
      }
    } finally {
      if (g === gen) syncAiBtn(false);
    }
  }

  /// need_keys: 文案 + 一键去密钥页(配置完用浏览器返回即可回到本视图)。
  function showNeedKeys(message) {
    const el = $("sg-ai-msg");
    el.innerHTML = "";
    el.className = "sg-msg err";
    el.appendChild(h("span", null, message + " "));
    const go = h("button", "sg-btn sg-btn-mini", t("sgGoSettings"));
    go.type = "button";
    go.addEventListener("click", () => R.navigate("keys"));
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
      const job = {
        jobId: reply.job_id,
        status: "running",
        report: null,
        metrics: null,
        chart: null,
        logs: null,
        error: null,
        body: body,
        strategyLabel: strategy,
      };
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
  /// `strategy` 可以是策略 id(普通回测)或 id + "#sweep"(寻优作业, P2-7)。
  function startPolling(strategy, jobId, g) {
    if (pollTimer) clearInterval(pollTimer);
    const stillHere = () =>
      !!detailCtx && (detailCtx.id === strategy || strategy === detailCtx.id + "#sweep");
    const tick = async () => {
      let info;
      try {
        info = await R.api("/api/backtest/" + encodeURIComponent(jobId));
      } catch (err) {
        if (g !== gen || !stillHere()) return;
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
      if (g !== gen || !stillHere()) return;
      const job = jobs.get(strategy);
      if (!job || job.jobId !== jobId) {
        if (pollTimer) clearInterval(pollTimer);
        return;
      }
      job.status = info.status;
      job.report = info.report || null;
      job.metrics = info.metrics || null;
      job.chart = info.chart || null;
      job.logs = info.logs || null;
      job.error = info.error || null;
      renderJob(job);
      if (info.status !== "running" && pollTimer) clearInterval(pollTimer);
    };
    pollTimer = setInterval(() => {
      tick().catch(() => {});
    }, POLL_MS);
  }

  /// 三态渲染: running 转圈 / done 指标卡+权益曲线+可折叠文本报告 / error 原因+重试。
  /// 寻优作业(kind==="sweep")另走对比表渲染。
  function renderJob(job) {
    const box = $("sg-bt-result");
    destroyBtChart();
    box.innerHTML = "";
    if (job.kind === "sweep") {
      renderSweepResult(job);
      return;
    }
    if (job.status === "running") {
      const row = h("div", "sg-job running");
      row.appendChild(h("span", "sg-spinner"));
      row.appendChild(h("span", null, t("sgBtRunning")));
      box.appendChild(row);
      return;
    }
    if (job.status === "done") {
      box.appendChild(h("div", "sg-job-head sg-ok", t("sgBtDone")));
      const m = job.metrics;
      if (m) {
        renderMetricCards(box, m);
        pushHistory(m, job.strategyLabel || (detailCtx && detailCtx.id) || "");
        // 零成交不是"策略不好", 多为配置/条件问题 —— 明说, 不让用户对着平线猜。
        if ((m.total_trades ?? 0) === 0) {
          box.appendChild(h("div", "sg-hint", t("sgZeroTradeHint")));
        }
      }
      if (job.chart && job.chart.times && job.chart.times.length) {
        renderBtChart(box, job.chart);
      }
      // 039 FR-7: 逐笔平仓明细(无事件时如实提示, 不渲染空表)。
      if (job.chart) {
        renderClosedTrades(box, job.chart, m);
      }
      // 策略日志 (ctx:log): 让"0 成交/停机"有可解释原因 —— 缺 start_price 的 [FATAL] 就在其中。
      // 零成交或有 [FATAL] 时默认展开, 否则折叠(逐 tick 日志可能很长)。
      renderStrategyLogs(box, job.logs, m && (m.total_trades ?? 0) === 0);
      // 文本报告收进折叠块(仍与 CLI 同一字逐同源, 供细读)。
      const det = h("details", "sg-advanced");
      const sum = h("summary", null, t("sgReportFull"));
      det.appendChild(sum);
      const pre = h("pre", "sg-report");
      pre.textContent = job.report || "";
      det.appendChild(pre);
      box.appendChild(det);
      // 回测闭环(P0-3): 一键把结论带给 AI 修改卡。
      if (m) {
        const row = h("div", "sg-row-right");
        const btn = h("button", "sg-btn sg-btn-mini", t("sgSendToAi"));
        btn.type = "button";
        btn.addEventListener("click", () => sendResultToAi(m));
        row.appendChild(btn);
        box.appendChild(row);
      }
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

  /// 策略日志块 (2026-10-05): 折叠展示 `ctx:log` 输出。含 [FATAL]/停机 或零成交时默认展开,
  /// 让用户一眼看到"为什么没成交 / 为什么停机"(如内置香农缺 start_price 的 [FATAL])。
  function renderStrategyLogs(box, logs, forceOpen) {
    if (!logs || !logs.length) {
      if (forceOpen) {
        box.appendChild(h("div", "sg-msg", t("sgStrategyLogsEmpty")));
      }
      return;
    }
    const fatal = logs.some((l) => l.indexOf("[FATAL]") >= 0 || l.indexOf("停机") >= 0);
    const det = h("details", "sg-advanced sg-logs");
    if (fatal || forceOpen) det.open = true;
    det.appendChild(h("summary", null, t("sgStrategyLogs") + " (" + logs.length + ")"));
    det.appendChild(h("div", "sg-hint", t("sgStrategyLogsHint")));
    // 数据只经 textContent/createTextNode 注入 (项目前端纪律: 不 innerHTML 用户数据)。
    // [FATAL]/停机 行单独着色, 便于一眼定位停机原因。
    const pre = h("pre", "sg-report sg-logs-body");
    for (const line of logs) {
      if (line.indexOf("[FATAL]") >= 0 || line.indexOf("停机") >= 0) {
        const span = h("span", "sg-log-fatal");
        span.textContent = line + "\n";
        pre.appendChild(span);
      } else {
        pre.appendChild(document.createTextNode(line + "\n"));
      }
    }
    det.appendChild(pre);
    box.appendChild(det);
  }

  /// 指标卡(P0-2): 颜色遵循涨红跌绿(中国行情惯例); 中性值不着色。
  function renderMetricCards(box, m) {
    box.appendChild(h("div", "sg-mcards-title", t("sgMetrics")));
    const grid = h("div", "sg-mcards");
    const cards = [
      [t("sgEqVsBench"), fmtPct(m.equity_change_pct), signCls(m.equity_change_pct),
        m.benchmark_return_pct != null ? t("sgBenchShort") + " " + fmtPct(m.benchmark_return_pct) : ""],
      [t("sgNetPnl"), fmtNum(m.net_pnl), signCls(m.net_pnl), ""],
      [t("sgMaxDd"), fmtPct(-Math.abs(m.max_drawdown_pct || 0)),
        (m.max_drawdown_pct || 0) > 0 ? "down" : "flat", ""],
      [t("sgWinRate"), fmtPct(m.win_rate), "flat", ""],
      [t("sgTrades"), String(m.total_trades ?? 0), "flat",
        (m.rejected_count > 0 ? t("sgRejected") + " " + m.rejected_count : "")],
      [t("sgSharpe"), m.sharpe != null ? m.sharpe.toFixed(2) : "—", "flat", ""],
    ];
    for (const [label, value, cls, sub] of cards) {
      const card = h("div", "sg-mcard");
      card.appendChild(h("span", "sg-mcard-label", label));
      const v = h("span", "sg-mcard-value" + (cls ? " " + cls : ""), value);
      card.appendChild(v);
      if (sub) card.appendChild(h("span", "sg-mcard-sub", sub));
      grid.appendChild(card);
    }
    box.appendChild(grid);
  }

  /// 权益曲线 vs 标的价格(P0-2): 两者同 quote 计价共轴; 买卖点标记在价格线上。
  /// 039 增补: 基准(买入持有)对照线 + 逐点回撤副图 + 图例。
  function renderBtChart(box, chart) {
    const wrap = h("div", "sg-bt-chart-wrap");
    // 039: 图例 —— 颜色与线一致(CSS 变量), 免去"这条虚线是什么"的猜测。
    const legend = h("div", "sg-bt-legend");
    for (const [cls, key] of [
      ["lg-price", "sgLegPrice"],
      ["lg-equity", "sgLegEquity"],
      ["lg-bench", "sgLegBench"],
      ["lg-dd", "sgLegDd"],
    ]) {
      legend.appendChild(h("span", "lg " + cls, t(key)));
    }
    wrap.appendChild(legend);
    const div = h("div", "sg-bt-chart");
    wrap.appendChild(div);
    box.appendChild(wrap);
    const border = themeColor("--border", "#2b333d");
    const LWC = window.LightweightCharts;
    btChart = LWC.createChart(div, {
      autoSize: true,
      layout: { background: { color: "transparent" }, textColor: themeColor("--muted", "#9aa7b4") },
      grid: { vertLines: { color: border }, horzLines: { color: border } },
      timeScale: { timeVisible: true, secondsVisible: false, borderColor: border },
    });
    const sec = (ms) => Math.floor(ms / 1000);
    // 价格线(muted 细线)。
    const price = btChart.addLineSeries({
      color: themeColor("--muted", "#9aa7b4"),
      lineWidth: 1,
      priceLineVisible: false,
      lastValueVisible: false,
    });
    price.setData(
      chart.times
        .map((t, i) => ({ time: sec(t), value: chart.price[i] }))
        .filter((d) => Number.isFinite(d.value) && Number.isFinite(d.time))
        .sort((a, b) => a.time - b.time)
    );
    // 权益线(accent 粗线)。
    const eq = btChart.addLineSeries({
      color: themeColor("--accent", "#4dd4ac"),
      lineWidth: 2,
      priceLineVisible: false,
    });
    eq.setData(
      chart.times
        .map((t, i) => ({ time: sec(t), value: chart.equity[i] }))
        .filter((d) => Number.isFinite(d.value) && Number.isFinite(d.time))
        .sort((a, b) => a.time - b.time)
    );
    // ---- 039 FR-5: 基准线(买入持有, 自首次成交起同本金) ----
    // 与指标卡 benchmark_return_pct 同一口径, 由后端算好下发; 建仓前的点是 null →
    // 只喂 time(whitespace 点), 曲线自然从建仓那根开始, **不**画成 0。
    if (chart.benchmark && chart.benchmark.length) {
      const bench = btChart.addLineSeries({
        color: themeColor("--warn", "#e3b341"),
        lineWidth: 1,
        lineStyle: 2, // 虚线: 一眼区别于策略权益与标的价格
        priceLineVisible: false,
        lastValueVisible: false,
      });
      bench.setData(
        chart.times
          .map((t, i) => {
            const v = chart.benchmark[i];
            return Number.isFinite(v) ? { time: sec(t), value: v } : { time: sec(t) };
          })
          .filter((d) => Number.isFinite(d.time))
          .sort((a, b) => a.time - b.time)
      );
    }
    // ---- 039 FR-6: 逐点回撤(水下曲线) ----
    // 值 ≤ 0, 0 在副图顶部; 由后端用**全分辨率**曲线算完再抽稀(口径 = 指标卡的 max_drawdown)。
    if (chart.drawdown && chart.drawdown.length) {
      const dd = btChart.addAreaSeries({
        priceScaleId: "dd",
        lineColor: themeColor("--error", "#f85149"),
        topColor: "transparent",
        bottomColor: themeColor("--error", "#f85149"),
        lineWidth: 1,
        priceLineVisible: false,
        lastValueVisible: false,
        priceFormat: { type: "custom", formatter: (v) => (v * 100).toFixed(1) + "%" },
      });
      btChart.priceScale("dd").applyOptions({ scaleMargins: { top: 0.78, bottom: 0 } });
      dd.setData(
        chart.times
          .map((t, i) => ({ time: sec(t), value: chart.drawdown[i] }))
          .filter((d) => Number.isFinite(d.value) && Number.isFinite(d.time))
          .sort((a, b) => a.time - b.time)
      );
    }
    // 买卖点(涨红跌绿: 买=红 上箭头, 卖=绿 下箭头)。
    const buyColor = themeColor("--error", "#f85149");
    const sellColor = themeColor("--accent", "#4dd4ac");
    const markers = (chart.fills || [])
      .map((f) => ({
        time: sec(f.t),
        position: f.side === "buy" ? "belowBar" : "aboveBar",
        color: f.side === "buy" ? buyColor : sellColor,
        shape: f.side === "buy" ? "arrowUp" : "arrowDown",
        text: f.side === "buy" ? "B" : "S",
      }))
      .sort((a, b) => a.time - b.time);
    if (markers.length) price.setMarkers(markers);
    btChart.timeScale().fitContent();
  }

  /// 039 FR-7/FR-8: 平仓盈亏明细表 —— 逐笔列出平仓事件时刻与已实现盈亏。
  ///
  /// 明细与指标卡**同源**(引擎一次 record_pnl 既进聚合也进明细), 故可给出可核对的一致性副标:
  /// 未被截断时 求和 == 已实现盈亏; 被截断时明说"仅显示最近 N 条(共 M 条)"并只报**显示部分**的和,
  /// 不许拿部分和冒充总数(诚实性硬约束)。
  function renderClosedTrades(box, chart, m) {
    const rows = chart.closed || [];
    if (!rows.length) {
      // 0 行不渲染空表 —— 直接说清"没有平仓事件"。
      box.appendChild(h("div", "sg-hint", t("sgClosedEmpty")));
      return;
    }
    const total = Number.isFinite(chart.closed_total) ? chart.closed_total : rows.length;
    const truncated = total > rows.length;
    box.appendChild(h("div", "sg-mcards-title", t("sgClosedTitle") + " (" + rows.length + ")"));
    const wrap = h("div", "sg-closed-wrap");
    const tbl = h("table", "sg-hist-table");
    const thead = h("thead");
    const hr = h("tr");
    hr.appendChild(h("th", null, t("sgClosedThTime")));
    hr.appendChild(h("th", null, t("sgClosedThPnl")));
    thead.appendChild(hr);
    tbl.appendChild(thead);
    const tbody = h("tbody");
    // 最新一笔在最上面(与"刚跑完想看最近发生了什么"一致); 数据仍是时间升序下发, 这里倒序渲染。
    for (const row of rows.slice().reverse()) {
      const tr = h("tr");
      tr.appendChild(h("td", null, fmtMs(row.t)));
      const cls = row.pnl > 0 ? "up" : row.pnl < 0 ? "down" : "";
      tr.appendChild(h("td", cls, fmtNum(row.pnl)));
      tbody.appendChild(tr);
    }
    tbl.appendChild(tbody);
    wrap.appendChild(tbl);
    box.appendChild(wrap);
    const shownSum = rows.reduce((acc, r) => acc + (Number.isFinite(r.pnl) ? r.pnl : 0), 0);
    let note = "";
    if (truncated) {
      note =
        t("sgClosedTrunc").replace("{n}", String(rows.length)).replace("{m}", String(total)) +
        " · " +
        t("sgClosedShownSum") +
        fmtNum(shownSum);
    } else {
      note = t("sgClosedSum") + fmtNum(shownSum);
      // 未截断时与指标卡交叉核对; 对不上如实提示(不静默)。
      if (m && Number.isFinite(m.realized_pnl) && Math.abs(m.realized_pnl - shownSum) > 0.01) {
        note += " · " + t("sgClosedMismatch") + fmtNum(m.realized_pnl) + ")";
      }
    }
    box.appendChild(h("div", "sg-closed-sub", note));
  }

  function destroyBtChart() {
    if (btChart) {
      btChart.remove();
      btChart = null;
    }
  }

  /// 图表配色跟随主题(同 markets.js 的取法)。
  function themeColor(name, fallback) {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  }

  function fmtPct(v) {
    if (v == null || !Number.isFinite(v)) return "—";
    return (v > 0 ? "+" : "") + v.toFixed(2) + "%";
  }

  function fmtNum(v) {
    if (v == null || !Number.isFinite(v)) return "—";
    const abs = Math.abs(v);
    const digits = abs >= 100 ? 2 : abs >= 1 ? 4 : 6;
    return (v > 0 ? "+" : "") + v.toFixed(digits).replace(/\.?0+$/, "");
  }

  /// 毫秒时间戳 → 展示串 (平仓明细用)。会话列表的秒级口径走 `R.formatTime`, 两者不可混
  /// (与 chat.js 的 `formatStamp` 同款格式, 但各视图各持一份 —— 那两份本来就不同口径)。
  function fmtMs(ms) {
    if (!Number.isFinite(ms)) return "—";
    const locale = R.lang === "en" ? "en-US" : "zh-CN";
    return new Date(ms).toLocaleString(locale, {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  function signCls(v) {
    if (v == null || !Number.isFinite(v) || v === 0) return "flat";
    return v > 0 ? "up" : "down";
  }

  /// 回测结论带给 AI(P0-3 闭环): 把关键指标拼成一句修改请求填进 AI 指令框。
  function sendResultToAi(m) {
    const parts = [
      "刚完成的回测结果: 权益变化 " + fmtPct(m.equity_change_pct),
      "买入持有基准 " + (m.benchmark_return_pct != null ? fmtPct(m.benchmark_return_pct) : "无"),
      "最大回撤 " + fmtPct(Math.abs(m.max_drawdown_pct || 0)),
      "夏普 " + (m.sharpe != null ? m.sharpe.toFixed(2) : "无"),
      "胜率 " + fmtPct(m.win_rate),
      "成交 " + (m.total_trades ?? 0) + " 笔",
    ];
    const ta = $("sg-ai-instr");
    ta.value = parts.join(", ") + "。请基于这份结果改进策略(例如先提高收益/降低回撤/减少无效交易)。";
    ta.scrollIntoView({ block: "center" });
    ta.focus();
  }

  // ---------- 回测历史对比 (P1-6) ----------

  /// 只进内存(会话级), 不写浏览器持久存储(SC-011); 最多 20 条。
  function pushHistory(m, strategyLabel) {
    jobHistory.unshift({
      ts: Date.now(),
      strategy: strategyLabel,
      pair: m.pair,
      interval: m.interval,
      days: m.days,
      equity_change_pct: m.equity_change_pct,
      benchmark_return_pct: m.benchmark_return_pct,
      max_drawdown_pct: m.max_drawdown_pct,
      sharpe: m.sharpe,
      win_rate: m.win_rate,
      total_trades: m.total_trades,
    });
    if (jobHistory.length > 20) jobHistory.pop();
    renderHistory();
  }

  function renderHistory() {
    const box = $("sg-bt-history");
    if (!box) return;
    box.innerHTML = "";
    box.appendChild(h("div", "sg-hist-title", t("sgHistory")));
    if (!jobHistory.length) {
      box.appendChild(h("div", "sg-empty", t("sgHistoryEmpty")));
      return;
    }
    const table = h("table", "sg-hist-table");
    const thead = h("thead");
    const hr = h("tr");
    for (const label of [
      t("sgHistoryTime"), t("sgName"), t("sgHistoryWindow"), t("sgEqVsBench"),
      t("sgMaxDd"), t("sgSharpe"), t("sgWinRate"), t("sgTrades"),
    ]) {
      hr.appendChild(h("th", null, label));
    }
    thead.appendChild(hr);
    table.appendChild(thead);
    const tbody = h("tbody");
    for (const r of jobHistory) {
      const tr = h("tr");
      const time = new Date(r.ts);
      const hh = String(time.getHours()).padStart(2, "0");
      const mm = String(time.getMinutes()).padStart(2, "0");
      const cells = [
        hh + ":" + mm,
        r.strategy,
        r.pair + " " + r.interval + " " + r.days + "d",
        fmtPct(r.equity_change_pct),
        fmtPct(Math.abs(r.max_drawdown_pct || 0)),
        r.sharpe != null ? r.sharpe.toFixed(2) : "—",
        fmtPct(r.win_rate),
        String(r.total_trades ?? 0),
      ];
      cells.forEach((c, i) => {
        const td = h("td", null, c);
        if (i === 3) td.classList.add(signCls(r.equity_change_pct));
        tr.appendChild(td);
      });
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    box.appendChild(table);
  }

  // ---------- 参数寻优 (P2-7) ----------

  /// 发起单参数网格扫描: 每档 = 一次完整回测(与单次回测同内核同口径), 结果进对比表。
  async function onSweep() {
    if (!detailCtx) return;
    const param = $("sg-sw-param").value.trim();
    const raw = $("sg-sw-values").value.trim();
    if (!param || !raw) {
      setMsg($("sg-sw-msg"), t("sgSweepEmpty"), "err");
      return;
    }
    const values = raw
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
      .map((s) => (Number.isFinite(Number(s)) ? Number(s) : s));
    const body = {
      strategy: detailCtx.id,
      param: param,
      values: values,
      pair: $("sg-bt-pair").value.trim() || $("sg-pair").value.trim(),
      interval: $("sg-bt-interval").value,
      days: Number($("sg-bt-days").value.trim()) || 90,
    };
    const fee = Number($("sg-bt-fee").value.trim());
    if (Number.isFinite(fee) && $("sg-bt-fee").value.trim() !== "") body.fee = fee;
    const cash = Number($("sg-bt-cash").value.trim());
    if (Number.isFinite(cash) && $("sg-bt-cash").value.trim() !== "") body.cash = cash;
    const leverage = Number($("sg-bt-leverage").value.trim());
    if (Number.isFinite(leverage) && $("sg-bt-leverage").value.trim() !== "") body.leverage = leverage;

    const g = gen;
    const btn = $("sg-sw-go");
    btn.disabled = true;
    setMsg($("sg-sw-msg"), "", "");
    try {
      const reply = await R.api("/api/backtest/sweep", { method: "POST", body: body });
      if (g !== gen) return;
      const key = detailCtx.id + "#sweep";
      jobs.set(key, { jobId: reply.job_id, status: "running", kind: "sweep", body: body });
      startPolling(key, reply.job_id, g);
    } catch (err) {
      if (g !== gen) return;
      setMsg($("sg-sw-msg"), (err && err.message) || String(err), "err");
    } finally {
      if (g === gen) btn.disabled = false;
    }
  }

  /// 寻优结果对比表: 权益变化最高档高亮; 单档失败原因如实入表。
  function renderSweepResult(job) {
    const box = $("sg-sw-result");
    box.innerHTML = "";
    if (job.status === "running") {
      const row = h("div", "sg-job running");
      row.appendChild(h("span", "sg-spinner"));
      row.appendChild(h("span", null, t("sgSweepRunning")));
      box.appendChild(row);
      return;
    }
    if (job.status === "error") {
      box.appendChild(h("div", "sg-msg err", job.error || ""));
      return;
    }
    let rows = [];
    try {
      rows = JSON.parse(job.report || "[]");
    } catch (_) {
      rows = [];
    }
    if (!rows.length) return;
    let best = null;
    for (const r of rows) {
      if (r.metrics && Number.isFinite(r.metrics.equity_change_pct)) {
        if (best === null || r.metrics.equity_change_pct > best.metrics.equity_change_pct) best = r;
      }
    }
    const table = h("table", "sg-hist-table");
    const thead = h("thead");
    const hr = h("tr");
    for (const label of [
      t("sgSweepValue"), t("sgEqVsBench"), t("sgNetPnl"), t("sgMaxDd"),
      t("sgSharpe"), t("sgWinRate"), t("sgTrades"),
    ]) {
      hr.appendChild(h("th", null, label));
    }
    thead.appendChild(hr);
    table.appendChild(thead);
    const tbody = h("tbody");
    for (const r of rows) {
      const tr = h("tr");
      if (r === best) tr.classList.add("sg-best");
      if (r.error) {
        const td = h("td", null, String(r.value));
        td.colSpan = 7;
        const err = h("span", "sg-msg err", r.error);
        td.appendChild(err);
        tr.appendChild(td);
      } else {
        const m = r.metrics || {};
        const cells = [
          String(r.value),
          fmtPct(m.equity_change_pct),
          fmtNum(m.net_pnl),
          fmtPct(Math.abs(m.max_drawdown_pct || 0)),
          m.sharpe != null ? m.sharpe.toFixed(2) : "—",
          fmtPct(m.win_rate),
          String(m.total_trades ?? 0),
        ];
        cells.forEach((c, i) => {
          const td = h("td", null, c);
          if (i === 1) td.classList.add(signCls(m.equity_change_pct));
          tr.appendChild(td);
        });
      }
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    box.appendChild(table);
    setMsg($("sg-sw-msg"), t("sgBtDone"), "ok");
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
      metrics: null,
      chart: null,
      logs: null,
      error: null,
      body: job.body,
      strategyLabel: job.strategyLabel || detailCtx.id,
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
      renderHistory(); // 会话级历史表(P1-6): 列表态与详情态都可见
      if (param) enterDetail(param, g);
      else enterList(g);
    },
    deactivate: () => {
      gen += 1; // 在途响应/轮询全部作废
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = 0;
      }
      destroyBtChart();
      $("sg-modal-root").innerHTML = "";
    },
  };
})();
