# 技术方案: 037 实盘资金安全加固

**功能目录**: `specs/changes/037-live-safety-hardening`

**创建日期**: 2026-10-06

**状态**: 已收敛 (2026-10-06; 见 [converge.md](converge.md))

**依据**: 宪法(原则二分层铁律 / 安全要求章) · `specs/architecture.md` §四(策略层)与 §七(安全模型) · `specs/research/framework-vs-commercial-2026-10.md` §四

---

## 决策清单(实施前定稿)

- **D1 接管策略**: 本实例归属挂单在启动时**一律撤销**(不提供 adopt 模式)。
  - 理由: 平台既有语义已是"本实例归属 = 本实例责任, 停机撤尽"(011 D6 归属前缀); 启动时存在归属挂单 ⟺ 上一轮非正常退出,**必然是残留**。加 `orphan_policy` 配置 = 重新长出配置面, 与 019-R5/`order_guard` 的"固定常量、不读配置"先例冲突。
- **D2 残留处置**: live **拒绝启动**; demo 警告继续; dry_run 不涉及(无交易所挂单)。
  - 理由: 与 `check_clock_skew` 拒绝启动同口径 —— 安全前置不成立时不进实盘; demo 不涉真钱, 不阻断体验。
- **D3 事件 kind**: 新增 `KIND_ORPHAN_CANCELED = "orphan_canceled"`(不改既有 kind 语义, 不升 `EVENTS_SCHEMA_VERSION` —— 加 kind 属**加法**, 与"加字段可以"同口径)。
- **D4 OMS 落点**: 纯逻辑模块 `crates/ricow_engine/src/oms.rs`(**不在** `ricow_strategy`), 因为它是**引擎视角**的会话账目; 接线全在 `command.rs` 的 `run_live` 主循环。**引擎已持有 `req`/`ack`**, 无需改 `LiveContext`。
- **D5 OMS 键**: 用 ack 的**最终** `client_order_id`(已含归属前缀)作键 —— 与用户流回报同源, 可精确对账; 策略提供的原始 id 不参与登记。
- **D6 P0-D 不做**: 见 spec §三/§六。

---

## 一、改动清单(按文件)

### 1. `crates/ricow_strategy/src/events.rs`
- 加 `pub const KIND_ORPHAN_CANCELED: &str = "orphan_canceled";` + 注释。
- 不动 schema 版本、不动 `RunEvent` 字段。

### 2. `crates/ricow_engine/src/live.rs`(纯逻辑)
- 新增:
  ```rust
  /// 按归属前缀切分挂单 (P0-A): (本实例归属, 非归属)。空前缀一律不归属 (宁漏撤不误撤)。
  pub fn split_owned(orders: &[OrderInfo], prefix: &str) -> (Vec<OrderInfo>, Vec<OrderInfo>);
  ```
- `plan_cleanup` 改用 `split_owned`(零行为变化, 既有用例即回归)。
- 新增 `OrphanOutcome`(撤单结果 + 残留)与 `HasResidual` 判定, 复用 `CleanupOutcome::record_cancel` 语义 —— 直接复用 `CleanupOutcome` 即可, 不新增重复类型。
- 新增纯判定:
  ```rust
  /// 启动接管后是否必须拒绝启动 (P0-A / D2): live 且仍有本实例残留挂单 → 拒绝。
  pub fn must_refuse_start(mode_is_live: bool, residual_owned: &[String]) -> bool
  ```
- 单测: 切分 / 拒绝判定 / 空前缀 / 空列表。

### 3. `crates/ricow_engine/src/oms.rs`(新文件, 纯逻辑)
- `OrderState`(Submitted/Open/PartiallyFilled/Filled/Canceled/Rejected/Unknown) + `is_terminal()`
- `OrderEntry { client_order_id, exchange_order_id, pair, side, size, filled_size, state, submitted_at_ms, updated_at_ms }`
- `OrderRegistry`:
  - `record_ack(&mut self, ack: &OrderAck, now_ms: i64) -> RecordVerdict`(`New` / `DuplicateLive`)
  - `record_unknown(&mut self, pair, side, size, cid_hint, reason, now_ms)`
  - `apply_user_order(&mut self, cid, status, filled_size, now_ms) -> bool`
  - `apply_fill(&mut self, cid, cum_filled, now_ms) -> bool`
  - `mark_canceled(&mut self, cid, now_ms)`
  - `unknown_ids(&self) -> Vec<String>`, `live_ids(&self)`(非终态), `counts() -> OmsCounts`
- 状态转移规则: 终态不可再变(除 Unknown→ 已知); 非法转移**如实降级为 warn 并保留原状态**(不静默修正)。
- 单测: 全状态转移矩阵 / 重复检测 / Unknown 收尾 / 部分成交累计。

### 4. `crates/ricow_engine/src/command.rs`(接线)
- `RunOutcome` 加 `pub orphan: Option<CleanupOutcome>` 与 `pub oms: OmsCounts`。
- 启动段(替换现 1042-1056 的"只 warn"块):
  1. `get_open_orders` → `split_owned` → 逐个 `cancel_order` → 复查 `get_open_orders` → `residual_owned`;
  2. 记 `outcome.orphan`;
  3. 事件流逐笔 `orphan_canceled`;
  4. `must_refuse_start(is_live, &residual)` → `return Err(CoreError::Exchange(指引))`。
- 主循环接线 OMS:
  - `ctx.place_order(req)` → `Ok(ack)`: `registry.record_ack`; 命中重复 → `outcome.oms.duplicates += 1` + warn + 事件。
  - `Err(e)`: `registry.record_unknown` + warn + 事件 + `outcome` 计数。
  - `UserEvent::Order/Fill` 分支: `registry.apply_*`。
  - 停机: `outcome.oms` 收尾; `Unknown` 非空 → 醒目提示。
- 常数: 不新增(纯逻辑无阈值)。

### 5. P0-C(只读聚合) —— 视 A/B 完成情况决定是否本轮落地
- 若落地: 在 `crates/ricow/src/commands/` 增加**只读**聚合函数(从 `db` 的 `positions`/`orders` 汇总), 由既有 `status`/`list` 输出; 不新增写路径。数据源本地库。

---

## 二、执行顺序(小步 + 每步验证)

1. events.rs 加 kind → `cargo build -p ricow_strategy`
2. live.rs: `split_owned` + `must_refuse_start` + 单测 → `cargo test -p ricow_engine`
3. oms.rs 新建 + 单测 → `cargo test -p ricow_engine`
4. command.rs 接线(P0-A 先, P0-B 后) → `cargo test -p ricow_engine`
5. 全量门禁: `cargo test --workspace --no-fail-fast` / `fmt --check` / `clippy -D warnings` / `scripts/ci_grep_gates.sh` / `architecture_guard`
6. 同步 `specs/architecture.md`(§七 安全模型 + §六 数据布局)与 `specs/roadmap.md`(变更档案状态表 + 测试基线)

---

## 三、边界与不做

- 不改 `plan_cleanup` 的既有行为(仅内部提 `split_owned`, 逐位等价)。
- 不改 `LiveContext` / `DryRunContext` 的任何签名(接线全在引擎侧)。
- 不新增 CLI 子命令与写动作(避免确认面变更)。
- **验证方式 = 纯逻辑单测**(不引入任何 Exchange 替身)。宪法原则三"交易流程禁 mock Exchange 替身"是**不可协商**的; 尽管仓内已有 `FundingStubExchange`/`ReconcileStubExchange` 这类**仅覆盖非交易本地分支**的先例, 本变更仍选择**更保守**的路径: 把全部判定收敛为纯函数(`split_owned` / `must_refuse_start` / `oms::OrderRegistry`), 单测直接打纯函数; `command.rs` 的接线由**编译 + 既有全量回归**兜底, 不新造替身、不发任何真实或模拟订单。
