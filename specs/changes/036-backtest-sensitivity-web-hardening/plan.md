# 036 计划

## 方案取向

1. **敏感性 = 编排层扫描, 不动引擎**。撮合内核零改动: 每档就是一次完整回测, 只是覆盖
   `slippage_bps` / `fee_maker_bps`+`fee_taker_bps` 两个参数。抽取 `BacktestOutcome`
   结构化结果让"读指标"不依赖文本解析 —— 这是唯一的结构性改动。
2. **SSE 续传复用 035 的 `LogTail` offset 语义**。事件 id = 文件字节 offset, 重连把它当起点,
   天然去重(不重发); 前端新打开器补查询参数续传。
3. **来源门放中间件, 不进各 handler**。与 token 同一道 `require_token`, 顺序先 token 后来源,
   401/403 都空体 —— 安全语义集中一处, 端点族无感知。
4. **拆文件按"端点族"切, 不按"层"切**。handler+请求体+测试同文件(与 keyring/markets 等 032 已拆模块同构);
   共享测试脚手架单独一个 `cfg(test)` 模块。

## 影响面

| 文件 | 变化 |
|---|---|
| `crates/ricow/src/commands/backtest.rs` | 抽 `BacktestOutcome` / `run_backtest_inner`; 新增阶梯解析、扫描编排、报告格式化与 4 组单测 |
| `crates/ricow/src/web/mod.rs` | 2178 → ~700 行, 只留骨架与装配 |
| `crates/ricow/src/web/{assets,auth,sessions,settings,trades,logs,strategies,test_support}.rs` | 新增(自 mod.rs 迁出 + FR-9/10/11 新能力) |
| `crates/ricow/src/web/{strategy_io,runs}/{mod,tests}.rs` | 目录化, 测试体下沉 |
| `crates/ricow/src/web/assets/{common,chat,runs}.js` | `R.sse` 统一打开器; 三处调用点改接 |
| `specs/{architecture,backtest}.md`, `specs/roadmap.md` | 同步 |

## 测试策略

- 单测: 阶梯解析(默认/显式/非法/单档报错)、轴结论(稳健/翻转点名)、显示宽度对齐、
  `is_loopback_origin` 变体矩阵、来源门读写分治、SSE 续传(`last_event_id` 参数不重发)。
- 回归: 未给敏感性开关时报告逐字一致; web 全组用例不改动断言(除迁移位置) —— 证明零行为变化。
- 真实联调: 沿用 `#[ignore]` 纪律, 本次改动无新增外部依赖路径, 不新增 ignore 用例。
