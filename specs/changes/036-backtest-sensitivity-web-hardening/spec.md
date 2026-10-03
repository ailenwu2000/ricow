# 036 回测敏感性 + Web 前端工程债加固

> 依据: `tmp/analysis/ricow-optimization-vs-vibe-trading-2026-10-03.md` 第四节(决策项, 用户拍板"增加")与第五节(用户拍板"修复")。
> 前序: 035 已落地分析文档第二/三节全部中高优项。

## 一、回测滑点/费用敏感性(分析文档第四节)

### 动机

回测里 `slippage_bps` 默认 0(理想成交), 手续费只是交易所挂牌价 —— 实盘冲击成本/滑点通常更高。
敏感性扫描直接回答"这个回测结论对成本假设有多敏感": **同一策略、同一数据窗口**, 沿一个成本轴跑多档,
看"成本后是否盈利"在哪一档翻转。不引入复杂撮合模型(YAGNI)。

### 需求

- FR-1: `ricow backtest --sensitivity [<bps,...>]` 沿**滑点轴**扫描; 省略档位 = 默认阶梯 `0,5,10`。
- FR-2: `ricow backtest --sensitivity-fee [<bps,...>]` 沿**费用轴**扫描(每档 maker=taker 同设, 与 `--fee` 同口径);
  省略档位 = 默认阶梯 `5,10,20`(BN 合约 taker / BN 现货基准 / 双倍)。
- FR-3: 两轴可同时给 —— 基准跑一次, 其余档位各跑一次完整回测(同一内核、同一缓存路径)。
- FR-4: 报告为定宽表格(显示宽度对齐, 中文列头不串行): 档位 / 成交 / 净盈亏 / 权益变化% / 最大回撤% / 夏普 / 胜率% / 手续费 / 结论;
  基准行标 `基准`, 并如实打印三层合并后的生效成本与数据来源。
- FR-5: 每轴给结论段: 阶梯内净盈亏符号一致 → 稳健(正/负各一档文案); 不一致 → 点名翻转处(`基准 → 10 bps`)。
- FR-6: 档位校验硬失败(中文): 非数字 / 负数 / 非有限值 / **少于 2 档**(单档不成敏感性)。
- FR-7: 敏感性扫描**不落 run card**(每档都落会污染归档目录); 只有用户不带开关的常规回测才落(035 §2.3 语义不变)。
- FR-8: 未给开关时报告文本与 035 逐字一致(零回归)。

### 实现要点

- 抽出结构化内核 `run_backtest_inner` → `BacktestOutcome { header, data_source, params, card, report }`;
  报告文本 / run card / 敏感性扫描三者共用同一内核, 敏感性**逐档读标量**, 不解析格式化文本。
- 纯函数 `parse_ladder` / `format_sensitivity_report` / `axis_summary` / `pad_display`(显示宽度) 便于单测。

## 二、Web 前端工程债(分析文档第五节)

### FR-9: 日志 SSE 断线续传 + 去重

- `/api/logs/{name}/stream` 事件带 id(= 文件字节 offset); 重连按 `Last-Event-ID` 头续传, **不重发已给过的行**。
- 前端统一 SSE 打开器(`R.sse`)重开的是**新** `EventSource` 对象, 浏览器不会自动带头 → 服务端同时接受
  `last_event_id` 查询参数(头优先, 同义)。
- 首屏照旧在响应前读好; 续传时首屏即"从 offset 起的增量", 不重复。

### FR-10: 前端统一 SSE 打开器(指数退避 + 抖动)

- `common.js` 新增 `R.sse(url, {onopen?, onmessage, onfail?})` → `{close()}`:
  浏览器原生重连**不退避也不封顶**, 这里接管 —— 出错即关掉重开, 1s→2s→4s→8s 封顶, ±20% 抖动;
  连续失败 5 次放弃并回调 `onfail` 一次; 任一次连上即清零重计。
- 记录最后一帧 `lastEventId`, 重开时拼 `&last_event_id=`(日志流续传, 会话流忽略)。
- 三处调用点(`chat.js` 会话流 / `chat.js` 日志面板 / `runs.js` 行内日志)全部改走 `R.sse`;
  `new EventSource` 从此只在 `common.js` 出现一处。

### FR-11: 写方法 Origin/Referer 来源门

- `require_token` 中间件内加第二道门(顺序: 先 token 后来源): `POST/PUT/PATCH/DELETE` 要求
  `Origin`(缺失退 `Referer`)为**回环来源**(`127.0.0.1` / `localhost` / `[::1]`, 忽略 userinfo/端口/路径);
  非回环一律 `403` 空体(不解释原因)。两者都没有 → 放行(非浏览器请求, curl/脚本/本仓测试)。
- 读方法不设限(`curl` / 脚本可用)。`Origin` 在场即以其为准, 不被回环 Referer 洗白。
- 防伪造变体: `127.0.0.1.evil.example` / `2130706433`(十进制 IP) / `ftp://127.0.0.1` / `null` 均不可信。

### FR-12: Web 模块拆文件(纯重构, 零行为变化)

- `web/mod.rs` 2178 行 → **~700 行**: 只留服务骨架(`bind` / `router` / `serve` / `Hub` / `WebState` / `WebError`)
  与端点装配; 端点族各归其位:
  - `assets.rs`(静态资产 + 前端断言测试) / `auth.rs`(token + 来源门, `ct_eq` 迁此并 `pub(super)`
    供 `runs` 风险确认短语复用) / `sessions.rs` / `settings.rs`(语言+主题, `current_lang` 迁此) /
    `trades.rs` / `logs.rs` / `strategies.rs`; 各自带 `routes()` 与自己的测试。
  - 共享测试脚手架(裸 HTTP 客户端 / 临时目录 / 种数据)下沉 `web/test_support.rs`(仅 `cfg(test)` 编译)。
- `strategy_io.rs`(1060) / `runs.rs`(886) 转目录模块, 测试体下沉 `strategy_io/tests.rs`(561) / `runs/tests.rs`(468);
  主体(非测试)各约 500 / 420 行, 内部本就按"请求体 / 纯函数 / handler"分区, 不再细拆(YAGNI)。
- 路由装配不变: 全部端点仍挂同一道 token 中间件之后; `mod.rs` 测试模块只留
  bind / 401 不泄漏 / Hub 单会话 / 兜底行语言 / 市场鉴权五组骨架用例。

## 非目标(显式不做)

- token 改 HttpOnly cookie: 长线项(分析文档原话), 本变更只补来源门。
- Web 前端构建工具链 / TS 化: 分析文档第六节明确不建议。
- 敏感性多轴组合扫描(滑点×费用网格): 单轴已能回答"翻转在哪", 网格属过度设计。
