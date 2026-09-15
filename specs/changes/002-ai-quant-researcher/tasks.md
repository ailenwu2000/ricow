# 002 任务分解

> 状态: **全部完成** (2026-09-13) | 基线: 294 → **301 passed / 0 failed / 11 ignored**

## 阶段一: 引擎侧(纯逻辑, 可单测)

- [x] **T001** `locus_engine::live::dry_run_gate(started_at: Option<i64>, now: i64, min_hours: f64) -> Result<(), String>`
      —— 关闭(≤0)/ 从未运行 / 不足 / 满足 四态; 单测四例 + 边界(恰好等于阈值)。
- [x] **T002** `locus_engine::strategy::execute_strategy(db, preview_id, token, dir)` —— 消费 token(不可绕过)
      → TOML 解析 → **同名冲突拒绝** → 写 `<name>.lua` + `<name>.toml`(script_path 指向) → 返回 (toml_path, lua_path)。
- [x] **T003** 单测: 未批准 token 不能部署 / 一次性 token 二次部署失败 / 同名冲突拒绝且**不产生半成品文件** /
      产物 TOML 不含 `script =`(R1)且 `script_path` 正确 / lua 内容与提交一致。

## 阶段二: CLI 入口

- [x] **T004** `locus create`: `--name` `--pair` `[--script <file|->]` `[--param k=v ...]` `[--market spot|futures]`
      —— 读码(文件/管道/stdin)→ `create_strategy_preview` → 打印回测报告(复用 backtest.rs 的报告打印口径)
      → 打印 preview_id + 下一步(`locus approve <id>` → `locus deploy <id> --token <t>`)。
- [x] **T005** `locus deploy <preview_id> --token <token>` —— 调 `execute_strategy` → 打印两个产物路径 + `locus run <name>` 建议。
- [x] **T006** 注册子命令(`main.rs` + `commands/mod.rs`), `--help` 文本写清三步链路。

## 阶段三: Dry Run 起点 + 实盘时长门禁

- [x] **T007** Dry Run 启动时写 `dry_run_started_at`(缺失才写): 实现在 run.rs / ctrl.rs 的 Dry Run 分支, 落盘用 `to_toml`
      (写盘前按 R1 处理 `script`); 打印一行"已记录 Dry Run 起点"。
- [x] **T008** 实盘分支前置 `dry_run_gate`(run.rs + ctrl.rs): 拒绝即返回错误(不降级为 Dry Run —— 声明了实盘却被降级是隐患)。
- [x] **T009** 单测: TOML 写回保留其它字段(`test_toml_roundtrip` 同口径); 已存在 `dry_run_started_at` 不被改写。

## 阶段四: 验证与文档

- [x] **T010** 全量测试 + 真实链路实测(SC-003) + 负例实测(SC-004/SC-005)。
- [x] **T011** 文档同步(SC-006) + `converge.md`。

---

## 实施记录 (2026-09-13)

**引擎侧**:
- `locus_engine::live::dry_run_gate(started_at: Option<&str>, now: DateTime<Utc>, min_hours: f64)`(纯函数)
  + `parse_dry_run_started`(RFC3339 优先, 兼容无时区按 UTC) + `DEFAULT_MIN_DRY_RUN_HOURS = 24.0`
- `locus_engine::strategy::execute_strategy(db, preview_id, token, dir)` —— `confirm::consume` 原子消费 token
  → `from_toml` 解析 → 同名冲突拒绝 → 路径穿越拒绝 → 写 `<name>.lua` + `<name>.toml`(`script_path` 指向),
  写 TOML 失败回收 `.lua` 不留半成品
- 顺带修正: `Engine::backtest_and_preview` 初始余额硬编码 `"USDC"` → `"USDT"`(全项目口径为 USDT, 报告币种原本是错的)

**CLI**:
- `locus create`(新): 读码(文件/stdin/AI 响应全文)→ `create_strategy` 门禁 → 按市场拉真实 K 线(现货 REST / 合约 fapi 公共)
  → `Engine::backtest_and_preview` → 打印报告 + `preview_id` + 三步指引
- `locus deploy`(新): 调 `execute_strategy` → 打印两个产物路径 + Dry Run/实盘建议
- 报告打印提取为 `commands::print_backtest_report`(backtest 与 create 共用, 避免格式漂移)
- `run.rs` / `ctrl.rs`: 实盘分支前置 `dry_run_gate`(`ctrl` 在**发起 daemon 请求前**同步拒绝);
  Dry Run 分支首次写入 `dry_run_started_at`(`record_dry_run_start`, 写副本并摘除注入的 `script`, 原文不内嵌)

**单测(294 → 301, +7)**:
- `test_dry_run_gate_disabled_when_non_positive` / `_rejects_never_ran` / `_rejects_insufficient_and_accepts_boundary`(含恰好 24h 边界与无时区解析) / `_custom_threshold_and_bad_timestamp`
- `test_execute_strategy_requires_approved_token`(未批准零落盘 / 批准后成功 / 产物无内嵌 script / 二次部署失败)
- `test_execute_strategy_refuses_overwrite`(拒绝覆盖且不留 `.lua`)
- `test_execute_strategy_rejects_path_traversal_name`

**真实链路实测(SC-003/004/005)**:
1. 语法错误 Lua → `错误: 生成代码未通过编译门禁: ... unexpected symbol near 'end'`, `strategies/` 目录**仍为空**
2. 正例 `--param order_size=0.01 --days 7` → 真实 K 线 168 根 → 报告(账户 100000 USDT, 标的 -0.74%)→ `preview_id`
3. 未批准 deploy → `preview 状态不是 approved: pending` ✓
4. `approve`(y)→ `deploy` → 产物 `ai-eth.toml`(含 `script_path`, **不含**内嵌 `script`)+ `ai-eth.lua`
5. 部署物 Dry Run 真跑: `tick=50 提交订单=1 成交=1`; 并打印 `已记录 Dry Run 起点到 ...`(TOML 落 `dry_run_started_at = "2026-09-13T12:22:25Z"`)
6. 一次性 token 二次部署 → `preview 状态不是 approved: consumed` ✓
7. 同名重复部署 → `同名策略已存在, 拒绝覆盖` ✓ 且目录未新增文件
8. `run --live` 与 `start --live` → 均 `Dry Run 累计 0.0 小时, 不足阈值 24.0 小时 —— 拒绝进实盘`, **未产生任何订单**
9. 边界: `min_dry_run_hours = 0` → 时长门禁放行, 随即被时钟预检拦住(证明只关了时长门禁, 其它护栏仍在)

**实施期踩到的坑(留档)**:
- `cargo test` 后 `target/debug/locus` **不是最新** —— 复验必须先 `cargo build`(否则会误判"功能没生效")
- `dry_run_started_at` 字段实际类型是 `Option<String>`(ISO8601), 不是时间戳数字; 先按 i64 写会编译不过
- python 批量替换时必须先做完删改**再**计算下标: 先删 `fmt_opt` 再用旧下标 splice, 会把补丁落进 `println!` 里(已回滚重做)
