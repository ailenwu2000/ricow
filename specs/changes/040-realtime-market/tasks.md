# 040 任务分解（tasks）

> 依赖顺序：T001 → T002 → T003 → T004 → T005 → T006 → T007 → T008。

- [x] **T001** `ricow_binance`：K 线帧解析（纯函数）
  - `ws.rs` 增 `parse_kline_frame`；`KLINE_READ_IDLE` 常量。
  - 单测：真实现货 / 合约帧各一份 + 缺 `k` / 非 kline 事件 / 数值类型错 → `None`。
    （本机对合约 K 线流零帧，故合约帧 fixture 按官方字段规约手写，注释已标明「真机帧见 T008 归因」，不冒充实测。）
- [x] **T002** `ricow_binance`：K 线流订阅（现货 + 合约）
  - `ws.rs` 增 `run_kline_ws`（退避重连 + 消费端退出即停）；`backoff_delay` 提 `pub(crate)`。
  - `BinanceClient::subscribe_klines`；`FuturesClient::subscribe_klines`；删 `futures_ws.rs` 的重复退避。
- [x] **T003** `web/realtime.rs`：订阅复用内核（`MarketHub` / `Sub` / `Lease`）
  - 注入式上游来源（便于单测）；单测锁定 FR-6 / FR-7 / FR-8 三条语义。
- [x] **T004** `web/realtime.rs`：SSE 端点与帧格式
  - `hello` / `kline` / `depth` / `error` 帧；`routes()`；分路隔离；20 档截断；单测锁帧口径。
- [x] **T005** `web/mod.rs`：装配（`WebState.realtime` + 路由 merge）+ `markets.rs` 三个解析函数提 `pub(super)`。
- [x] **T006** `markets.js`：SSE 接入 + 增量更新 + 三态状态 chip + 生命周期收口。
- [x] **T007** 门禁与单测：fmt / clippy / ci_grep_gates / node --check / 全量 `cargo test`。
  - 结果：fmt 0 差异；clippy **exit 0、零告警**（同轮早前曾出现 17 条 Windows 增量锁文件噪音，属环境非代码）；grep 红线 5/5；9 份 JS 过；全量 **829 passed / 0 failed / 22 ignored**。
- [x] **T008** 真机取证 + 文档收尾（architecture / roadmap / converge / 记忆）。
  - 探针 `tmp/probe_040_stream.py` → **23 PASS / 0 FAIL**；裸 WS 归因 `tmp/probe_futkline_raw.mjs`（合约 K 线零帧**非**我方 URL/参数问题）。
  - 文档：`specs/architecture.md`（040 段 + 基线）+ `specs/roadmap.md`（档案行 + 基线链）+ 本目录 `converge.md`。
