# 002 实施计划

**前置**: 011(实盘运行器, 已落地) / 004(风控, 已落地) —— 本变更接在实盘链路之后, 补的是**入口**与**时长条件**。

## 一、关键决策

| # | 决策 | 理由 |
|:--|:--|:--|
| D1 | CLI 形态: 新增扁平子命令 `locus create`(提交)与 `locus deploy`(落盘), 批准复用已有 `locus approve` | 与现有扁平命令集(`backtest`/`approve`)一致; 不引入子命令组(命令集文档已长, 不再加层级) |
| D2 | 部署物 = `strategies/<name>.toml`(`params.script_path = "<name>.lua"`)+ `strategies/<name>.lua` | 与既有 loader 口径一致(`script_path` 是文档化的正规路径), 用户可直接编辑 .lua; 不内嵌 script 字符串 |
| D3 | 部署逻辑落 `locus_engine::strategy::execute_strategy`(消费 preview token + 写文件), CLI 只解析参数/打印 | 文件写入可单测(临时目录), 不在 CLI 里堆逻辑; 与 008 以来"CLI 薄、引擎厚"一致 |
| D4 | 门禁判定落 `locus_engine::live::dry_run_gate`(纯函数), 在实盘启动分支(run/start)调用 | 纯函数可单测四种输入; 与 011 的 `live_gate`/`check_clock_skew` 同一处风格 |
| D5 | `min_dry_run_hours` 默认 **24**, `params.min_dry_run_hours = 0` 关闭 | 依据: 覆盖亚/欧/美三个交易时段的一个完整日周期; 换策略节奏的用户可下调 —— 不做无依据的保守放大 |
| D6 | 不新增 DB 表/列; Dry Run 起点用既有 TOML 字段 `dry_run_started_at` | 该字段本就是为此设计(`architecture.md` §十一 A 指向本变更接管), 最小机制 |

## 二、改动清单

| 文件 | 改动 |
|:--|:--|
| `crates/locus_engine/src/strategy.rs` | 新增 `execute_strategy(db, preview_id, token, dir)` — 消费 token → 解析 TOML → 校验同名不冲突 → 写 `<name>.toml` + `<name>.lua` → 返回部署路径; 单测(临时目录) |
| `crates/locus_engine/src/live.rs` | 新增 `dry_run_gate(started_at, now, min_hours) -> Result<(), String>`; 单测四例 |
| `crates/locus_engine/src/lib.rs` | 导出 `execute_strategy` / `dry_run_gate` |
| `crates/locus_cli/src/commands/create.rs`(新) | `locus create --name --pair [--script <file|->] [--param k=v] [--market]`: 读码 → `Engine::create_strategy_preview` → 打印回测报告 + preview_id + 下一步指引 |
| `crates/locus_cli/src/commands/deploy.rs`(新) | `locus deploy <preview_id> --token <t>`: 调 `execute_strategy` → 打印部署路径 + `locus run <name>` 建议 |
| `crates/locus_cli/src/main.rs` / `commands/mod.rs` | 注册两个子命令 |
| `crates/locus_cli/src/commands/run.rs` / `ctrl.rs` | ① 实盘分支前置 `dry_run_gate`; ② Dry Run 启动时写 `dry_run_started_at`(缺失才写, 已存在不改) |
| 文档 | `architecture.md` §五 命令集 + §七 现状注记 + §十一 A + §九 基线; `lua-api.md` 部署路径; `roadmap.md` 002 行; `product.md` §三.3 |

## 三、风险

| # | 风险 | 处置 |
|:--|:--|:--|
| R1 | 写回 TOML 时把 loader 注入的 `script` 内容(大段 Lua)写进文件 | 写盘前: 若 `params.script_path` 存在则移除内存中的 `script`; 单测断言产物里无 `script =` |
| R2 | 覆盖用户已部署的策略文件 | 默认拒绝(FR-005), 报错含现有路径; 不做静默改名/备份 |
| R3 | 时长门禁挡住正常使用 | 只在实盘路径生效; 默认 24h 可按策略下调; 拒绝消息给出准确解除方式; Dry Run/回测/沙箱完全不受影响 |
| R4 | `approve` 的交互式输入在非交互环境(stdin 非 tty)下拿不到确认 | 保持现状(用户在场批准), 本变更不加"免交互批准"开关 —— AI 不得自行批准 |

## 四、验收动作

1. `cargo test --workspace`(基线 294 → 预期 30x)。
2. 真实链路: `locus create` 提交一段真 Lua → 真实 K 线回测报告 → `approve`(y)→ `deploy` → 目录产物 + `locus run <name>` Dry Run 真跑起来。
3. 负例: 语法错误代码(拒绝且无文件); 同名重复部署(拒绝)。
4. 门禁负例: `dry_run_started_at` 刚写 → `run <name> --live` 与 `start <name> --live` 均拒绝(不产生订单)。
5. 文档同步检查。
