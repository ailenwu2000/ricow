// Ricow Web 前端「策略」视图 (032 US3, FR-015 ~ FR-021):
//   列表(市场 × 来源四组) / 复制内置 / 新建空白 / 详情(参数表单 + Lua 编辑器) /
//   保存(编译门禁, 运行中拒绝) / AI 改 Lua(草稿+撤销, need_keys 引导密钥页) / 回测异步作业(轮询三态)。
// 注册为 `R.views.strategies`; 全部数据走 R.api 的 `/api/strategies*` 与 `/api/backtest*`。
//
// 过期响应: 视图级"代际令牌" gen —— activate/deactivate 自增, 在途响应回来比对捕获值, 不一致即丢弃。
// 安全: 不写浏览器持久存储; 服务端数据一律 textContent / .value(不拼 innerHTML); 编辑器不输出 HTML。
// 042: 本文件是「策略」视图的**外壳** —— 命名空间 / 工具 / 常量 / 共享状态 / 列表态 /
// 新建与复制弹窗 / 视图注册。其余三份子文件(详情表单 / Lua 编辑器 / 回测)从 `R.sg` 取用:
//   strategy_form.js     详情态 + 参数表单 + 清单编辑器
//   strategy_editor.js   Lua 编辑器 + 保存(编译门禁) + AI 改写
//   strategy_backtest.js 回测作业 + 指标卡/图表/平仓明细 + 寻优 + 会话级历史
// **加载顺序即契约**: 本文件必须最先执行(见 assets.rs::ASSETS 与 index.html 注释)。
"use strict";

(function () {
  const R = window.Ricow;

  // ---------- 共享命名空间 (042 FR-3) ----------
  // 工具 / 常量 / 状态 / 跨文件符号只在这里定义一次, 三份子文件从 `R.sg` 取 ——
  // 不复制第二份, 否则"四份行为一致"要靠人去同步维护。
  const S = (R.sg = {});

  // ---------- 工具(唯一实现) ----------
  /// 取元素 / 双语文案 / 建元素 / 行内消息: 四份文件共用这一份实现, 子文件经 `S` 取用。
  const t = (key) => R.t(key);
  const $ = (id) => document.getElementById(id);

  /// 建元素: 文本一律 textContent(服务端数据即使含 <> 也只作文本)。
  function h(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  /// 静态双语标签属性(仅用于无插值的固定文案; 服务端文本绝不走这里)。
  function bi(key) {
    return ' data-zh="' + R.TEXT.zh[key] + '" data-en="' + R.TEXT.en[key] + '"';
  }

  /// 行内消息: text 空串清空; kind: "" | "ok" | "err"。
  function setMsg(el, text, kind) {
    el.textContent = text || "";
    el.className = "sg-msg" + (kind ? " " + kind : "");
  }

  S.t = t;
  S.$ = $;
  S.h = h;
  S.bi = bi;
  S.setMsg = setMsg;

  // ---------- 常量 ----------
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

  S.INTERVALS = INTERVALS;
  S.DEFAULT_INTERVAL = DEFAULT_INTERVAL;
  S.DEFAULT_PAIR = DEFAULT_PAIR;
  S.NAME_RE = NAME_RE;
  S.POLL_MS = POLL_MS;
  S.RESERVED_PARAM_KEYS = RESERVED_PARAM_KEYS;
  S.BLANK_LUA = BLANK_LUA;
  S.CHART_MAX_POINTS = CHART_MAX_POINTS;

  // ---------- 共享状态袋 (042 FR-4) ----------
  // 视图级可变状态全部收在这里, 三份子文件用同一个袋子 —— 尤其 `gen`(代际令牌):
  // 它的"作废在途响应"语义跨全部四份文件, 拆成多份绑定会让这条不变量静默失效。
  const st = (S.st = {
    mounted: false,
    // 代际令牌: 每次激活/离开自增, 过期响应一律不碰 DOM。
    gen: 0,
    listData: null, // 最近一次列表响应(切语言直接重画, 不重新取数)
    // 当前详情上下文: { id, detail, source, builtin }
    detailCtx: null,
    // 参数输入注册表(每次渲染详情重建): { key, ty, required, input }。
    paramInputs: [],
    savedCode: "", // 进入时/上次保存成功的代码(撤销目标)
    aiDraft: false, // 编辑器当前是否装着未落盘的 AI 草稿
    // 052: AI 卡当前模式 —— "params"(默认, 只填参数表单、不碰代码) / "code"(改写 Lua)。
    // 放状态袋而非只读 DOM, 是为了让"切语言 / 离开再回来"重建时仍保持同一模式。
    aiMode: "params",
    // N3: AI 对话线程(**两个模式各一条**, 切换模式不互相污染: 一个聊的是"参数取值该多少",
    // 一个聊的是"代码逻辑怎么改", 混在一条历史里会把对方的上下文带错)。
    // 每条是 { role:"user"|"ai", text / advice / changed / warnings, ... } 的数组。
    // 只活在内存: 刷新页面、切策略都清空 —— 服务端不存会话(端点是纯一次问答), 这一点在
    // 卡片上如实写着, 不让用户以为对话被记住了。
    aiThreads: { params: [], code: [] },
    saveLocked: false, // 服务端 409 running 后锁定保存钮
    // 回测作业台账(模块变量, 不跨刷新): strategyId -> { jobId, status, report, error, body }。
    jobs: new Map(),
    pollTimer: 0,
    // 寻优作业(P2-7): 同一台账的另一类条目, key = strategyId + "#sweep"。
    // 回测历史(P1-6, 仅本次会话内存; 不写浏览器持久存储)。
    jobHistory: [],
    // 当前图表实例(重渲染/离开前 remove, 同 markets.js 的图表生命周期)。
    btChart: null,
    // 清单编辑里的当前参数行(保存时逐行收集; 不含空行)。
    manifestRows: [],
  });

  // ---------- 跨文件符号 (042 D2: 延迟查找) ----------
  // 一律写成 `(...a) => S.fn(...a)`: 取"函数值"会快照加载期状态, 而外壳与子文件之间是
  // 双向依赖(mount 绑 onSave; onSave 调 enterDetail; enterDetail 又调 renderJob) ——
  // 用值别名会随加载顺序静默取到 undefined(绑到 undefined 不报错, 点了没反应)。
  const enterDetail = (...a) => S.enterDetail(...a);
  const renderBadges = (...a) => S.renderBadges(...a);
  const readParam = (...a) => S.readParam(...a);
  const paramRow = (...a) => S.paramRow(...a);
  const stringRow = (...a) => S.stringRow(...a);
  const addManifestRow = (...a) => S.addManifestRow(...a);
  const onSaveManifest = (...a) => S.onSaveManifest(...a);
  const onExtractParams = (...a) => S.onExtractParams(...a);
  const renderParamsGuide = (...a) => S.renderParamsGuide(...a);
  const updateDirty = (...a) => S.updateDirty(...a);
  const refreshHighlight = (...a) => S.refreshHighlight(...a);
  const syncSaveBtn = (...a) => S.syncSaveBtn(...a);
  const syncAiBtn = (...a) => S.syncAiBtn(...a);
  const setAiMode = (...a) => S.setAiMode(...a);
  const syncBtBtn = (...a) => S.syncBtBtn(...a);
  const onCodeKeydown = (...a) => S.onCodeKeydown(...a);
  const onUndo = (...a) => S.onUndo(...a);
  const onSave = (...a) => S.onSave(...a);
  const onAiEdit = (...a) => S.onAiEdit(...a);
  // N3: 清空对话线程(历史只在前端, 见 strategy_editor.js)。
  const onAiClearThread = (...a) => S.onAiClearThread(...a);
  const onBacktest = (...a) => S.onBacktest(...a);
  const onSweep = (...a) => S.onSweep(...a);
  // C8: 多维寻优的维度行在表单子文件里建/读(键列表与单参数下拉同源), 发起与结果在回测子文件里。
  const addOptDim = (...a) => S.addOptDim(...a);
  const onOptimize = (...a) => S.onOptimize(...a);
  const renderJob = (...a) => S.renderJob(...a);
  const renderHistory = (...a) => S.renderHistory(...a);
  const destroyBtChart = (...a) => S.destroyBtChart(...a);

  // ---------- 双语文案(并入公共字典; 静态标签走 data-zh/data-en) ----------
  Object.assign(R.TEXT.zh, {
    sgTitle: "策略",
    sgNew: "+ 新建空白",
    sgCopy: "复制",
    // N1 删除自定义策略。文案刻意把"保留什么"分开列 —— 删一个策略不该抹掉它的历史,
    // 但用户会以为"删干净了", 所以必须写在确认按钮之前。
    sgDelete: "删除",
    sgDeleteTitle: "删除策略",
    sgDeleteBody: "确定删除这个策略吗? 此操作不可撤销(脚本、实例配置、参数清单一起删掉)。",
    sgDeleteWhich: "策略: ",
    sgDeleteKeep: "下面这些会保留 —— 删策略不等于抹掉历史:",
    sgDeleteKeepBacktest: "回测历史(run/backtest/ 下的报告)",
    sgDeleteKeepLedger: "台账与成交记录(ricow.db)",
    sgDeleteOk: "确认删除",
    sgDeleting: "删除中…",
    sgDeleteDone: "已删除",
    sgOpen: "打开",
    sgBuiltin: "内置",
    sgUser: "我的",
    sgNoUserHint: "还没有自己的策略 —— 内置策略要先「复制」才能改; 也可以直接新建空白。",
    sgCopiedName: "已复制为 ",
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
    // N3 (2026-10-12): 「调整参数」从"把一句话翻译成键值"改成**多轮对话 + 说意图**。
    // 占位文字也按模式分开了 —— 参数模式提示的是"说效果"(用户本来就不该被迫说出参数名),
    // 代码模式提示的才是"说逻辑"。两者共用一句占位会误导其中一个模式。
    sgAiPhParams: "说说你想要的效果, 例如「保守一点」「更看好震荡行情」—— 不需要指定改哪个参数、改成多少",
    sgAiPhCode: "用一句话说明策略逻辑怎么改, 例如「加一个跌破均线就清仓」",
    sgAiEmpty: "先说说你想达到什么效果。",
    sgAiBusy: "AI 处理中…",
    sgAiOk: "AI 草稿已生成(通过编译),确认无误后点「保存」才会落盘。",
    // 052: AI 卡分两种模式 —— 「调参数」只填参数表单, 「改代码逻辑」才动 Lua。
    // 默认「调参数」: 用户的困惑正是"不知道该配什么", 那就默认走**不碰代码**的那条路。
    sgAiModeParams: "调整参数",
    sgAiModeCode: "改代码逻辑",
    sgAiModeHintParams:
      "多轮对话: 你只说想要的效果(如「保守点」「更看好震荡行情」), AI 自己判断该动哪些参数、动到什么程度, 并讲清理由与代价; 它只填上面的参数表单、不改代码, 核对后点「保存」才写盘。",
    sgAiModeHintCode:
      "用一句话说明策略逻辑怎么改,例如「加一个跌破均线就清仓」—— AI 会改写下面的 Lua 代码(仍是草稿, 保存才落盘)。",
    sgAiGoParams: "按意图调参",
    sgAiGoCode: "改代码逻辑",
    sgAiParamsOk: "已按 AI 的建议填进上面的参数表单(那只是草稿)—— 核对后点「保存」才会写入。",
    sgAiParamsEmpty:
      "AI 建议的参数都落在本页参数表单之外(可能指向脚本本身), 没有可填的内容 —— 换一种说法再试。",
    // N3 对话线程与改动对照。
    // (注: 字典块里的注释**不能**写成 `N3:` 这种「标识符+冒号」形态 ——
    //  前端检查脚本会把注释也当字典键解析, 于是凭空多出一个只缺英文定义的假键。)
    sgAiThreadNote: "对话只留在当前页面: 刷新页面或切换策略后不再保留。",
    sgAiClear: "清空对话",
    sgAiUserLabel: "你",
    sgAiBotLabel: "AI",
    sgAiChangedTitle: "这次改了",
    sgAiChangedNone: "这次没有改动任何取值 —— 上面的说明就是结论, 参数保持原样。",
    sgAiChangedDefault: "改前值只是清单默认值(你还没有保存过它)",
    // N3 B 段 —— 自动评估改动影响(两次同窗口回测 + 实测指标解读)。
    sgAiAssessTitle: "评估这次改动的实际影响",
    sgAiAssessBody:
      "用默认窗口(90 天 · 1h)把「改前: 上次保存的参数」与「改后: 表单里现在这些」各跑一次回测, 再让 AI 按两份实测指标讲清影响 —— 两次同窗口才可比。",
    sgAiAssessRun: "跑回测评估影响",
    sgAiAssessRunningBase: "正在回测「改前」参数…",
    sgAiAssessRunningNew: "正在回测「改后」参数…",
    sgAiAssessAssessing: "两次回测都完成, AI 正在读结果…",
    sgAiAssessTimeout: "等待回测超时(10 分钟): 作业可能还在跑, 可到下方回测卡继续看结果。",
    sgAiAssessWindow: "回测窗口: 90 天 · 1h",
    sgAiAssessFailed: "评估没做成: ",
    sgAiAssessBusy: "这个策略已经有回测在跑: 等它结束后再试(同一策略同时只允许一个回测作业)。",
    sgAiAssessStale: "页面已切换(换了策略或离开了本页), 本次评估中止。",
    // 052: 未声明清单时的参数引导(只有一句结论的红字, 用户不知道该干什么)。
    sgParamsGuideTitle: "这个策略还没声明参数",
    sgParamsGuideBody:
      "参数表单是照「参数清单」画的, 而这份策略还没有清单 —— 每个参数都没有中文名和说明, 所以这里基本是空的。可以从脚本里把参数认出来(键 / 类型 / 默认值自动填好), 或者先补好清单再让 AI 告诉你怎么配。",
    sgParamsGuideExtract: "从脚本识别参数",
    sgParamsGuideAskAi: "问 AI 怎么配",
    sgParamsGuideAskAiFirst:
      "AI 得先有一份参数清单, 才能告诉你每个参数该怎么配。请点左边「从脚本识别参数」, 核对后点「保存清单」, 再回来问它。",
    sgOtherParamsHint: "来自实例 TOML, 未在清单里声明 —— 「从脚本识别参数」可以把它们变成正式声明的参数。",
    sgMfExtract: "从脚本识别参数",
    sgMfExtracting: "识别中…",
    // N2 (2026-10-12): 这条文案改口了。原先写"请补上中文名与说明" —— 那正是用户抱怨的负担
    // (他是脚本的使用者, 不是作者, 不可能知道作者把旋钮叫什么)。现在中文名可以留空。
    sgMfExtractOk: "已识别出参数(键 / 类型 / 默认值已填好); 脚本注释里写过的说明也一并带出。中文名留空也能保存 —— 想要更好的名字, 可点「让 AI 起中文名」。",
    sgMfExtractNone: "这份脚本里没识别到参数读取(它可能不读任何参数)。",
    // N2-C 复核: 这四条原先叫 `sgMfName*` —— 与下面清单卡的「策略名」字段**同名**。
    // JS 对象字面量里重名的键后者胜, 于是按钮与回写文案都变成了「策略中文名」那个值;
    // 顺带 `id="sg-mf-name"` 也与那个文本输入重了(取到的是先渲染的那个 input, 按钮反而没绑上)。
    // 改名前缀为 `sgMfAutoName*` 一并解决两处撞车。
    sgMfAutoName: "让 AI 起中文名",
    sgMfAutoNaming: "起名中…",
    sgMfNameOk: "AI 已按脚本代码填上中文名与说明(只填空位, 你填过的没动), 核对后点「保存清单」。",
    sgMfNameNone: "没有可填的空位 —— 这些参数的中文名/说明都已经有了。",
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
    // 适用性卡片(帮助计划第 7 条): 清单里早就写好 适用/不适用 场景, 此前只在「清单编辑器」
    // (新建/复制时)可见, 只读详情页一个字都不显示 —— 8 个内置策略只能靠名字猜。
    // 这里把同一份数据在**只读**详情页展示出来, 数据源 = GET /api/strategies/{id} 的 manifest,
    // 后端零改动。
    sgDetail: "详情",
    sgFit: "适用性",
    sgFitSummary: "一句话说明",
    sgFitDesc: "长说明",
    sgFitSuitable: "适用场景",
    sgFitUnsuitable: "不适用场景",
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
    // C9: 与同策略同窗口的上一次结果并排对比(仅会话内, 服务端不存归档)。
    sgCmpTitle: "与上次对比",
    sgCmpHint: "同一策略、同一窗口: 本次与上一次并排 —— 变化按涨红跌绿着色。",
    sgCmpMetric: "指标",
    sgCmpPrev: "上次",
    sgCmpNow: "本次",
    sgCmpDelta: "变化",
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
    // C8: 多维寻优(多参数笛卡尔积)。与单参数扫描并列, 但进的是另一个端点。
    sgOpt: "多维寻优",
    sgOptHint:
      "沿多个参数同时扫: 每组 = 一次完整回测, 组合数上限 100。维度越多组合数涨得越快, 建议先 2 维。",
    sgOptValues: "候选值(逗号分隔, 2–10 个)",
    sgOptAdd: "添加维度",
    sgOptDel: "删除该维度",
    sgOptGo: "开始寻优",
    sgOptRunning: "寻优运行中…",
    sgOptEmpty: "至少填一个维度: 参数键 + 候选值。",
    sgOptMaxDims: "最多 4 个维度。",
    sgOptProgress: "进度",
    sgOptBest: "最优(按",
    sgOptBestSuffix: ")",
    sgOptSortHint: "点表头排序; 失败的行始终排在最后。",
    sgOptErrRow: "该组失败",
    sgWarnings: "静态检查提示",
    sgApiRef: "ctx API 速查",
    sgSavedWithWarn: "已保存(附静态检查提示)",
  });
  Object.assign(R.TEXT.en, {
    sgTitle: "Strategies",
    sgNew: "+ New blank",
    sgCopy: "Copy",
    sgDelete: "Delete",
    sgDeleteTitle: "Delete strategy",
    sgDeleteBody: "Delete this strategy? This cannot be undone (script, instance config and parameter manifest are all removed).",
    sgDeleteWhich: "Strategy: ",
    sgDeleteKeep: "The following is kept — deleting a strategy does not erase its history:",
    sgDeleteKeepBacktest: "Backtest history (reports under run/backtest/)",
    sgDeleteKeepLedger: "Ledger and fills (ricow.db)",
    sgDeleteOk: "Delete",
    sgDeleting: "Deleting…",
    sgDeleteDone: "Deleted",
    sgOpen: "Open",
    sgBuiltin: "Built-in",
    sgUser: "Mine",
    sgNoUserHint:
      "No strategies of your own yet — copy a built-in to modify it, or start from a blank one.",
    sgCopiedName: "Copied as ",
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
    sgAiPhParams: "Describe the effect you want, e.g. “more conservative” or “I expect range-bound markets” — no need to name a parameter or a value",
    sgAiPhCode: "Say what to change in the strategy logic, e.g. “exit when price breaks below the moving average”",
    sgAiEmpty: "First say what you want to achieve.",
    sgAiBusy: "AI working…",
    sgAiOk: "AI draft ready (compiles). Press Save to write it to disk.",
    sgAiModeParams: "Tune parameters",
    sgAiModeCode: "Change code",
    sgAiModeHintParams:
      "Multi-turn chat: describe the effect you want (e.g. “more conservative”, “I expect range-bound markets”). The AI decides which parameters to move, by how much, and explains the reasoning and the cost. It only fills the parameter form above — never the code. Press Save to write.",
    sgAiModeHintCode:
      "Describe the logic change in one sentence, e.g. “exit all when price drops below the moving average”. The AI rewrites the Lua below (still a draft; save to write it).",
    sgAiGoParams: "Adjust by intent",
    sgAiGoCode: "Change code",
    sgAiParamsOk: "AI suggestions are filled into the parameter form above (a draft) — review them and press Save to write.",
    sgAiParamsEmpty:
      "The AI's suggested parameters all fall outside this page's parameter form (they may point at the script itself), so there is nothing to fill in — try rephrasing.",
    sgAiThreadNote: "This conversation lives in the current page only: a refresh or switching strategies clears it.",
    sgAiClear: "Clear chat",
    sgAiUserLabel: "You",
    sgAiBotLabel: "AI",
    sgAiChangedTitle: "Changed this round",
    sgAiChangedNone: "No value was changed this round — the note above is the conclusion; the parameters stay as they are.",
    sgAiChangedDefault: "the “before” value is only the manifest default (you never saved it)",
    sgAiAssessTitle: "Measure the real impact of this change",
    sgAiAssessBody:
      "Run two backtests over the default window (90 days · 1h) — “before: last saved parameters” and “after: the values currently in the form” — then let the AI read both measured metric sets. The shared window is what makes them comparable.",
    sgAiAssessRun: "Backtest and assess",
    sgAiAssessRunningBase: "Backtesting the “before” parameters…",
    sgAiAssessRunningNew: "Backtesting the “after” parameters…",
    sgAiAssessAssessing: "Both backtests finished — the AI is reading the results…",
    sgAiAssessTimeout: "Timed out waiting for the backtests (10 minutes): they may still be running — check the backtest card below.",
    sgAiAssessWindow: "Backtest window: 90 days · 1h",
    sgAiAssessFailed: "The assessment did not happen: ",
    sgAiAssessBusy: "A backtest is already running for this strategy. Wait for it to finish and try again (only one backtest job per strategy at a time).",
    sgAiAssessStale: "the page changed (different strategy or left this view), so this assessment stopped.",
    sgParamsGuideTitle: "This strategy declares no parameters",
    sgParamsGuideBody:
      "The parameter form is drawn from the strategy manifest, and this strategy has no manifest yet — so the parameters have no names or descriptions and the form is basically empty. You can detect them from the script (keys / types / defaults filled in automatically), or fill in the manifest first and then ask the AI how to configure it.",
    sgParamsGuideExtract: "Detect parameters from script",
    sgParamsGuideAskAi: "Ask the AI how to configure",
    sgParamsGuideAskAiFirst:
      "The AI needs a parameter manifest before it can tell you how to configure each parameter. Click “Detect parameters from script”, review, press “Save manifest”, then come back and ask.",
    sgOtherParamsHint: "From the instance TOML, not declared in the manifest — “Detect parameters from script” can promote them to declared parameters.",
    sgMfExtract: "Detect from script",
    sgMfExtracting: "Detecting…",
    sgMfExtractOk: "Parameters detected (keys / types / defaults filled in); descriptions written in script comments come along too. Names may be left blank — press \"Name them with AI\" if you want better ones.",
    sgMfExtractNone: "No parameter reads were detected in this script (it may not read any parameters).",
    sgMfAutoName: "Name them with AI",
    sgMfAutoNaming: "Naming…",
    sgMfNameOk: "AI filled in names and descriptions from the script code (blank fields only; your own entries were left alone). Review, then press Save manifest.",
    sgMfNameNone: "Nothing to fill — every parameter already has a name or description.",
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
    sgDetail: "Details",
    sgFit: "Suitability",
    sgFitSummary: "Summary",
    sgFitDesc: "Description",
    sgFitSuitable: "Good for",
    sgFitUnsuitable: "Not suitable for",
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
    // C9: side-by-side vs. the previous run of the same strategy + window (session-only).
    sgCmpTitle: "Compared with last run",
    sgCmpHint:
      "Same strategy and window: this run side by side with the previous one — deltas follow up=red / down=green.",
    sgCmpMetric: "Metric",
    sgCmpPrev: "Previous",
    sgCmpNow: "This run",
    sgCmpDelta: "Change",
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
    // C8: multi-param grid search (Cartesian product).
    sgOpt: "Multi-param search",
    sgOptHint:
      "Sweep several params at once: each combo is a full backtest, capped at 100 combos. Combos grow fast — start with 2 dims.",
    sgOptValues: "Candidates (comma separated, 2–10)",
    sgOptAdd: "Add dim",
    sgOptDel: "Remove this dim",
    sgOptGo: "Run search",
    sgOptRunning: "Search running…",
    sgOptEmpty: "Fill at least one dim: param key + candidates.",
    sgOptMaxDims: "At most 4 dims.",
    sgOptProgress: "Progress",
    sgOptBest: "Best by ",
    sgOptBestSuffix: "",
    sgOptSortHint: "Click a header to sort; failed rows always sink to the bottom.",
    sgOptErrRow: "This combo failed",
    sgWarnings: "Static check hints",
    sgApiRef: "ctx API reference",
    sgSavedWithWarn: "Saved (with static check hints)",
  });

  // ---------- 一次性挂载(骨架只建一次; 数据每次激活重取) ----------

  function mount() {
    if (st.mounted) return;
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
      // 适用性卡片(帮助计划第 7 条): 内容由 renderDetail → renderFit 填(数据取自 manifest);
      // 清单没写这几栏时整卡隐藏, 不占位。
      '<section class="sg-card sg-fit-card" id="sg-fit-card" hidden>' +
      '<div class="sg-card-head"><span' + bi("sgFit") + ">" + R.TEXT.zh.sgFit + "</span></div>" +
      '<div class="sg-card-body" id="sg-fit-body"></div></section>' +
      '<div class="sg-detail-grid">' +
      // 参数
      '<section class="sg-card"><div class="sg-card-head"><span' + bi("sgParams") + ">" +
      R.TEXT.zh.sgParams + "</span></div>" +
      // 052 方案4: 清单为空时, 表单只能画出交易对 —— 用户看到"就一个框"却不知该干什么。
      // 这块引导卡由 strategy_form.js::renderParamsGuide 按**服务端事实**(manifest.params 为空)决定显隐,
      // 两个按钮分别把用户引到"从脚本识别参数"与"问 AI 怎么配"。清单里有参数时整块隐藏, 不占位。
      // 注意: 必须放在 `#sg-params` **外面** —— renderParams 每次 `innerHTML = ""` 会把它一起抹掉。
      '<div id="sg-params-guide" class="sg-params-guide" hidden></div>' +
      "<div id='sg-params' class='sg-card-body'></div></section>" +
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
      '<div class="sg-row-right">' +
      // 052 方案2: 从 Lua 静态提取读取到的参数键, 一键预填上面这些行 —— 用户不必手动对着代码抄。
      '<button id="sg-mf-extract" class="sg-btn sg-btn-mini" type="button"' +
      bi("sgMfExtract") + ">" + R.TEXT.zh.sgMfExtract + "</button>" +
      // N2-C: 让 AI 按**脚本代码**给这些键起中文名与写说明。同样不落盘 —— 只填进上面的行,
      // 用户核对后点「保存清单」才写。
      '<button id="sg-mf-autoname" class="sg-btn sg-btn-mini" type="button"' +
      bi("sgMfAutoName") + ">" + R.TEXT.zh.sgMfAutoName + "</button>" +
      '<button id="sg-mf-add" class="sg-btn sg-btn-mini" type="button"' +
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
      // 052: AI 卡分两种模式 —— 「调参数」只填上面的参数表单(不碰代码)、「改代码逻辑」才动 Lua。
      // 默认停在「调参数」: 用户的困惑正是"不知道该配什么", 先给一条**不动代码**的路。
      // 模式 chip 的静态标签直接走 `bi()`; 「执行」按钮与提示语的文案随模式变 —— 由
      // strategy_editor.js::setAiMode 同时改写 data-zh/data-en(否则切语言会被旧模式文案盖回去)。
      '<div class="sg-seg" id="sg-ai-modes" role="group">' +
      '<button id="sg-ai-mode-params" class="sg-seg-btn on" type="button"' +
      bi("sgAiModeParams") + ">" + R.TEXT.zh.sgAiModeParams + "</button>" +
      '<button id="sg-ai-mode-code" class="sg-seg-btn" type="button"' +
      bi("sgAiModeCode") + ">" + R.TEXT.zh.sgAiModeCode + "</button></div>" +
      '<div id="sg-ai-mode-hint" class="sg-ai-hint"></div>' +
      // N3: 对话线程(用户气泡 / AI 气泡 + 改动对照)。历史只在这个 DOM 里, 也只在内存里 ——
      // 服务端不存会话(见 strategy_editor.js 的说明), 所以下面那行提示必须如实写明"刷新即失"。
      '<div id="sg-ai-thread" class="sg-ai-thread"></div>' +
      '<div id="sg-ai-thread-note" class="sg-hint"' + bi("sgAiThreadNote") + ">" +
      R.TEXT.zh.sgAiThreadNote + "</div>" +
      '<textarea id="sg-ai-instr" class="sg-ai-instr" rows="3" spellcheck="false"' +
      // 占位文字随模式变 —— 由 setAiMode 一并改写(与按钮文案同一纪律: data 属性也要写, 否则
      // 切语言时 applyLang 会把它换回初始模式那一句)。
      ' data-zh-placeholder="' + R.TEXT.zh.sgAiPhParams + '" data-en-placeholder="' +
      R.TEXT.en.sgAiPhParams + '"></textarea>' +
      '<div class="sg-row-right">' +
      '<button id="sg-ai-clear" class="sg-btn sg-btn-mini" type="button"' +
      bi("sgAiClear") + ">" + R.TEXT.zh.sgAiClear + "</button>" +
      '<button id="sg-ai-go" class="sg-btn" type="button">' +
      R.TEXT.zh.sgAiGoParams + "</button></div>" +
      '<div id="sg-ai-msg" class="sg-msg" aria-live="polite"></div>' +
      // N3-B: 改动影响评估面板。默认收起 —— 只有"这次真的改了值"时才由
      // strategy_editor.js 展开(没改动却摆一个按钮, 用户点了只会得到 400)。
      '<div id="sg-ai-assess" class="sg-ai-assess" hidden></div></section>' +
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
      // C8 多维寻优: 维度行由 JS 动态建(可增删), 键选项与上面的单参数下拉同源。
      '<details class="sg-advanced sg-sweep-box" id="sg-opt-box"><summary' + bi("sgOpt") + ">" +
      R.TEXT.zh.sgOpt + "</summary>" +
      "<div class='sg-opt-hint'" + bi("sgOptHint") + ">" + R.TEXT.zh.sgOptHint + "</div>" +
      '<div id="sg-opt-dims"></div>' +
      '<div class="sg-row-right"><button id="sg-opt-add" class="sg-btn sg-btn-mini" type="button"' +
      bi("sgOptAdd") + ">" + R.TEXT.zh.sgOptAdd + "</button>" +
      '<button id="sg-opt-go" class="sg-btn" type="button"' +
      bi("sgOptGo") + ">" + R.TEXT.zh.sgOptGo + "</button></div>" +
      '<div id="sg-opt-msg" class="sg-msg" aria-live="polite"></div>' +
      '<div id="sg-opt-result"></div></details>' +
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
    // N3: 清空当前模式的对话线程(历史全在前端, 服务端没有可清的东西)。
    $("sg-ai-clear").addEventListener("click", () => onAiClearThread());
    // 052: AI 卡的模式 chip(默认「调参数」)。初始化必须走 setAiMode ——
    // 它同时把 chip 高亮 / 「执行」按钮标签 / 一行提示按 zh 或 en 写成一致的一套。
    $("sg-ai-mode-params").addEventListener("click", () => setAiMode("params"));
    $("sg-ai-mode-code").addEventListener("click", () => setAiMode("code"));
    setAiMode(st.aiMode);
    $("sg-bt-go").addEventListener("click", onBacktest);
    $("sg-sw-go").addEventListener("click", onSweep);
    // C8: 维度行**不在这里铺** —— 挂载时还没读策略详情, 参数清单是空的。等详情渲染后再
    // 按真实参数铺(syncOptDims, 见 strategy_form.js 的 renderDetail / refreshSchema)。
    $("sg-opt-add").addEventListener("click", () => addOptDim());
    $("sg-opt-go").addEventListener("click", onOptimize);
    $("sg-mf-add").addEventListener("click", () => {
      addManifestRow(null);
      setMsg($("sg-mf-msg"), "", "");
    });
    $("sg-mf-save").addEventListener("click", onSaveManifest);
    // 052 方案 2: 从编辑器里的 Lua 静态识别参数键 → 预填清单行(不落盘, 用户核对后保存清单)。
    $("sg-mf-extract").addEventListener("click", () => onExtractParams());
    // N2-C: 让 AI 按脚本代码给参数键起中文名与写说明(不落盘, 用户核对后保存清单)。
    $("sg-mf-autoname").addEventListener("click", () => onAiNameParams());
    st.mounted = true;
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
      st.listData = await R.api("/api/strategies");
      if (g !== st.gen) return;
      renderList(st.listData);
    } catch (err) {
      if (g !== st.gen) return;
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
    // B4: 一个自己的策略都没有时补一条"下一步"。四组里「内置」那两组**永远是满的**, 光看它们
    // 会得出"反正都有了"的错觉 —— 而用户最常卡住的恰恰是"想改内置策略, 却不知道得先复制"。
    if (!data.some((s) => s.source === "user")) {
      const hint = h("div", "sg-firststep");
      hint.appendChild(h("span", "sg-firststep-t", t("sgNoUserHint")));
      const btn = h("button", "sg-btn sg-btn-mini", t("sgNew"));
      btn.type = "button";
      btn.addEventListener("click", openNewModal);
      hint.appendChild(btn);
      body.appendChild(hint);
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
      // 内置: 可看详情(适用性 + 参数 + 直接回测), 但**不能直接改** —— 改要先「复制」。
      // 帮助计划第 7 条: 此前内置行只有「复制」一个入口, 详情页虽然后端早就支持
      // (`#strategies/{id}`, 清单里的 description/suitable/unsuitable 都在里面), 但用户
      // 根本走不进去 —— 列表主点击被绑成了"复制"。
      const view = h("button", "sg-btn sg-btn-mini", t("sgDetail"));
      view.type = "button";
      view.addEventListener("click", () => openDetail(s.id));
      act.appendChild(view);
      const btn = h("button", "sg-btn sg-btn-mini", t("sgCopy"));
      btn.type = "button";
      btn.addEventListener("click", () => onCopy(s).catch(strategyErr));
      act.appendChild(btn);
      main.addEventListener("click", () => openDetail(s.id));
    } else {
      // 用户: 打开详情编辑。
      const btn = h("button", "sg-btn sg-btn-mini", t("sgOpen"));
      btn.type = "button";
      btn.addEventListener("click", () => openDetail(s.id));
      act.appendChild(btn);
      // N1 自定义策略可删。内置行**没有**这个按钮 —— 服务端也会 409 reserved(双保险),
      // 但入口本身就不该出现: 让人点了才知道不行, 是无谓的往返。
      const del = h("button", "sg-btn sg-btn-mini sg-btn-danger", t("sgDelete"));
      del.type = "button";
      del.addEventListener("click", () => onDelete(s));
      act.appendChild(del);
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
    const builtins = (st.listData || []).filter((s) => s.source === "builtin");
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

    // 模板状态: 选中内置后缓存 {id, market, lua, detail}。
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
        tpl = { id: id, market: detail.market, lua: source.lua, detail: detail };
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
        // 052 方案 1: 选了模板就带 `copy_from` —— 让模板的**参数清单**随副本一起落盘。
        // 否则模板的参数只存在于内置清单里: 副本页面画不出参数行, 用户就"不知道该配什么"。
        // 空白骨架没有来源, 自然不带(tpl 为 null)。
        const body = {
          name: name,
          market: tpl ? tpl.market : marketSel.value,
          pair: pair,
          code: tpl ? tpl.lua : BLANK_LUA,
          params: params,
          overwrite: false,
        };
        if (tpl) body.copy_from = tpl.id;
        await R.api("/api/strategies", { method: "POST", body: body });
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
        const draft = await R.api("/api/strategies/ai-generate", {
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
            code: draft.code,
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

  // ---------- 删除自定义策略(N1, 2026-10-12) ----------

  /// 删除一个自定义策略: 弹确认框, 并**如实写明保留什么**。
  ///
  /// 保留项必须写在确认按钮**之前**: 回测历史与台账**不会**跟着删 —— 删一个策略不该顺手抹掉
  /// 它的历史, 那会让总收益这类统计跟着变。用户有权在点之前知道, 而不是删完才发现。
  function onDelete(s) {
    const { card, close } = openModal(t("sgDeleteTitle"));
    const form = h("div", "sg-modal-form");
    form.appendChild(h("div", "sg-modal-hint", t("sgDeleteBody")));
    const which = h("div", "sg-copy-from");
    which.appendChild(h("span", "sg-muted", t("sgDeleteWhich")));
    which.appendChild(h("span", null, s.name)); // 服务端数据只走 textContent, 绝不拼 HTML
    form.appendChild(which);
    form.appendChild(h("div", "sg-muted", t("sgDeleteKeep")));
    const keep = h("ul", "sg-del-keep");
    keep.appendChild(h("li", null, t("sgDeleteKeepBacktest")));
    keep.appendChild(h("li", null, t("sgDeleteKeepLedger")));
    form.appendChild(keep);
    const errLine = h("div", "sg-msg");
    form.appendChild(errLine);
    card.appendChild(form);

    const { cancel, primary } = modalActions(card, "sgDeleteOk");
    primary.classList.add("sg-btn-danger");
    cancel.addEventListener("click", close);
    primary.addEventListener("click", async () => {
      primary.disabled = true;
      cancel.disabled = true;
      primary.textContent = t("sgDeleting");
      try {
        await R.api("/api/strategies/" + encodeURIComponent(s.id), { method: "DELETE" });
        close();
        R.toast(t("sgDeleteDone"), "ok");
        // 重新拉列表: 让服务端说"现在还剩什么", 不在本地猜着删 DOM。
        await enterList(st.gen);
      } catch (err) {
        errLine.className = "sg-msg err";
        errLine.textContent = (err && err.message) || String(err);
        primary.disabled = false;
        cancel.disabled = false;
        primary.textContent = t("sgDeleteOk");
      }
    });
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
        // 052 方案 1: 带上 `copy_from` —— 服务端会把源策略的**参数清单**派生成副本自己的清单,
        // 否则副本只是一份裸 .lua: 参数表单画不出任何一行, 用户回到"不知道该配什么"。
        // 只有源**确实声明了清单**时才带(裸 .lua 的源服务端会 400 拒绝 —— 那种情况退回旧行为,
        // 让副本页的引导卡把用户引到「从脚本识别参数」)。
        const body = {
          name: name,
          market: s.market,
          pair: pair,
          code: lua,
          // 参数留空: 清单随行落盘后, 表单会按清单里的默认值预填(不再依赖 Lua 内 fallback)。
          params: {},
          overwrite: false,
        };
        if (s.declared) body.copy_from = s.id;
        await R.api("/api/strategies", { method: "POST", body: body });
        close();
        // B6: 这里会**从列表切到详情** —— 提示若写进列表的 msg 区, 用户刚好就看不到它。
        // 全局 toast 挂在 body 上, 跟着用户走过这次跳转。
        R.toast(t("sgCopiedName") + name, "ok");
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

  // ---------- 切语言: 动态文本就地刷新(静态标签由公共 applyLang 处理) ----------

  function refreshDynamic() {
    if (st.listData && !$("sg-listview").hidden) renderList(st.listData);
    if (st.detailCtx) {
      renderBadges(st.detailCtx.detail, st.detailCtx.source);
      $("sg-builtin-hint").textContent = t("sgBuiltinHint");
      // 052: 参数引导卡的文字是动态建的(没有 data-zh 可换), 切语言要就地重建一次。
      // 只重建这块 —— 参数行/清单行重建会把用户填了一半的值抹掉。
      renderParamsGuide(st.detailCtx.detail);
      updateDirty();
      syncSaveBtn(st.saveLocked ? "locked" : "idle");
      syncAiBtn(false);
      syncBtBtn(false);
      const job = st.jobs.get(st.detailCtx.id);
      if (job) renderJob(job);
    }
  }

  const baseApplyLang = R.applyLang;
  R.applyLang = function (lang) {
    const ret = baseApplyLang.call(this, lang);
    try {
      if (st.mounted) refreshDynamic();
    } catch (_) {
      // 视图尚未完成挂载时忽略。
    }
    return ret;
  };

  R.views.strategies = {
    /// param = 二级 hash 解码后的策略 id; 缺省 = 列表态。
    activate: (param) => {
      mount();
      const g = (st.gen += 1);
      R.applyLang(R.lang);
      $("sg-listview").hidden = !!param;
      $("sg-detail").hidden = !param;
      $("sg-modal-root").innerHTML = "";
      renderHistory(); // 会话级历史表(P1-6): 列表态与详情态都可见
      if (param) enterDetail(param, g);
      else enterList(g);
    },
    deactivate: () => {
      st.gen += 1; // 在途响应/轮询全部作废
      if (st.pollTimer) {
        clearInterval(st.pollTimer);
        st.pollTimer = 0;
      }
      destroyBtChart();
      $("sg-modal-root").innerHTML = "";
    },
  };
})();
