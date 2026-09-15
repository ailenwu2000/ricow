# 003 实施计划: 出站通知

## 一、决策

- **D1 通道 = 单个 webhook + 可选 chat_id**: Telegram 的 `sendMessage` 接受 `POST {"chat_id","text"}`, 通用 webhook 忽略多余字段 → 一个实现覆盖主流通道。不引入第三方 SDK、不写死厂商(`product.md` §四"本地优先")。
- **D2 放在 `locus_engine/src/notify.rs`**: 引擎已有 `reqwest`(拉 K 线等), 且事件产生点全在引擎循环内; 抽象(trait)与实现同处一层, 不制造跨 crate 回调。**不**放 locus_core(它没有网络依赖, 且通知不是核心领域概念)。
- **D3 配置落 `StrategyConfig.notify: Option<NotifyToml>`**: 与 `[backtest]` 段同构(现有模式), 不新增配置文件/环境变量面。
- **D4 默认关闭**: `enabled = false` 缺省 —— 与"实盘默认 Dry Run"一致的保守口径; 用户显式开启才出站。
- **D5 纯逻辑与 I/O 分离**: `NotifyEvent`(数据)+ `format_message`(文本)+ `Throttle`(白名单/限速/丢弃计数)+ 强平去重 全部可单测; `WebhookNotifier` 只做"把已格式化的文本 POST 出去"。
- **D6 失败隔离**: 每次投递 `tokio::spawn` + 5s 超时; 失败只 `warn`。**不重试、不排队**(A2: 通知不保证送达, 不得反压交易循环)。
- **D7 事件接线**: 成交(实盘用户流回写点 / Dry Run 撮合成交点)、熔断(引擎看到熔断类拒单时)、强平告警(014 的 `warn_near_liquidation`)、停机残留(清理结束后 `residual` 非空时)。

## 二、改动清单

1. `crates/locus_strategy/src/config.rs`: `NotifyToml { enabled, webhook, chat_id, events, min_interval_secs }` + `StrategyConfig.notify`
2. `crates/locus_engine/src/notify.rs`(新): `NotifyEvent` / `format_message` / `Throttle` / `Notifier` trait / `NoopNotifier` / `WebhookNotifier`
3. `crates/locus_engine/src/lib.rs`: 导出
4. `crates/locus_engine/src/command.rs`:
   - `run_live` / `run_dry_run` 构造 notifier(从 config), 接四类事件
   - 熔断类拒单识别(RiskError 熔断变体 → `circuit_breaker` 事件)
   - 强平告警去重集合
5. 单测(notify.rs + command.rs 接线路径)
6. 文档: product.md(§三.4 落地标注)、architecture.md、roadmap.md、本档案

## 三、风险

- **R1 通知刷屏**: 由限速 + 强平去重 + 成交类别可关控制(D5/D4)。
- **R2 网络抖动反压交易**: 由 spawn + 超时 + 不重试保证(D6)。
- **R3 配置字段扩散**: 只加 `StrategyConfig` 一个可选字段, 构造点用 `None` 兜底(与 `backtest` 同法)。
- **R4 真实通道未验证**(无用户侧 Telegram token): 用本地回环端点验证真实 HTTP 往返(SC-003), 并在档案中如实标注"未接真实第三方通道"。
