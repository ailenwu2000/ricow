# 037 收敛记录 (实盘资金安全加固)

**功能目录**: `specs/changes/037-live-safety-hardening`

**收敛日期**: 2026-10-06

**依据**: [`specs/research/framework-vs-commercial-2026-10.md`](../../research/framework-vs-commercial-2026-10.md) §四 —— 本轮实施 🔴 P0-A / P0-B / P0-C 三项; **P0-D 显式不做**(理由见 §决策记录与 spec §三)。

**用户指令**: "先实现P0级几个问题的、完善。注意，因为事关资金安全，请严格按照设计规范来实现代码。相关问题可以模拟验证，即使不能解决，但是要保证不能影响已实现功能。"

## 一、实跑门禁(2026-10-06, Windows, agent 环境)

| 门禁 | 结果 |
|:--|:--|
| `cargo fmt --all -- --check` | **0 差异** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0、零代码告警**(残余 warning 全为 Windows 增量编译锁文件 `os error 5` 环境噪声, 与既有基线记录一致) |
| `cargo deny --locked check` | 全绿 |
| `bash scripts/ci_grep_gates.sh` | **5 条安全红线全绿**(AI 工具层零落盘 / 明文密钥不进日志 / 无调试残留 / `execute_strategy` 调用点白名单 / `Instant` 无裸减法) |
| `cargo test --workspace --no-fail-fast` | **761 passed / 0 failed / 22 ignored** |
| `architecture_guard` | **3 / 3 全绿**(含规则 A 引擎+绑定层、规则 B CLI 层、清单↔Lua 键一致性) |

**测试增量 = +25**(与本变更一一对应, 其余 target 一字未变):

| target | 前 | 后 | 增量来源 |
|:--|--:|--:|:--|
| `ricow` bin | 370 | **382** | +12: `commands/exposure.rs` 的渲染 / 过滤 / 模式标签 / 显示宽度对齐 |
| `ricow_engine` lib | 99 | **112** | +13: `exposure.rs` 的只读聚合(跨策略对冲 / 模式隔离 / 名义低估标记 / 挂单计数 / 顺序稳定 / 纯函数) |
| ricow_binance 59 · ricow_core 18 · ricow_strategy 184 · `architecture_guard` 3 · `ai_live_smoke` 3 | — | — | 未变 |

> **如实**: 同口径上次记为 670(2026-10-05)。670 → 736 的差额来自 **2026-10-06 的四笔审计加固提交**(安全/资源泄漏/稳定性三梯队 + 中危 ×2 + 低危收尾), 那几轮只写了提交说明、未单独记基线 —— 故本轮起点按实跑 **736** 计, 736 + 25 = 761。

**不变量核对**(spec §不变量, 违反即回滚):

| 不变量 | 结论 |
|:--|:--|
| 架构铁律: 引擎/CLI/绑定层零策略参数名 | ✅ `architecture_guard` 3/3; 新增代码只用 `pair` / `mode` / `status` 这类通用键与结构字段名 |
| 既有测试全绿(基线不减) | ✅ 736 → 761, **0 failed** |
| 停机清理语义一字不改 | ✅ `plan_cleanup` 仅把内联切分换成 `split_owned`, 行为逐字不变(新单测覆盖切分口径) |
| 写操作确认面不变(不新增写动作) | ✅ 本轮新增的唯一动作是**推送**的撤单(启动接管), 无新写路径、无新确认面、无新配置面 |

## 二、端到端模拟验证(用户授权: "相关问题可以模拟验证")

**方式**: 临时数据目录(`RICOW_ROOT=tmp/verify_*`)+ 用 Python `sqlite3` 手工灌 `positions` / `orders` 行, 再跑真实二进制观察输出。
**刻意不做的**: 不连 testnet、**不引入任何 Exchange 替身**(宪法原则三"交易流程禁 mock Exchange 替身"不可协商); 本轮也不新增下单语义, 故无需真机交易链路。

### P0-C 组合敞口(`ricow exposure`)

灌入 6 条持仓 / 6 条订单(grid_eth 与 hedge_eth 在 ETHUSDT 上反向对冲、eth_dry 为 dry_run、mystery 有头寸无成本价、flat_one 已平仓、另有 filled/cancelled 订单应被排除), 实测:

| 场景 | 期望 | 实测 |
|:--|:--|:--|
| 跨策略对冲 | ETHUSDT/live 净头寸 0, 但多 1.5 / 空 −1.5 / 名义 5850 | ✅ 与手算一致(1.5×2000 + 1.5×1900) |
| 模式隔离 | ETHUSDT 的 live 与 dry_run **各占一行**, 绝不相加 | ✅ dry_run 单独一行(净 2 / 名义 5000) |
| 挂单计数 | 只数 `open` / `partially_filled`; `filled`/`cancelled` 不计 | ✅ grid_eth 2 笔 + btc_trend 1 + eth_dry 1 = 4 笔未终结(6 条订单中 2 条被排除) |
| 名义低估 | 有头寸无成本价 → 该行名义标 `*` + 脚注 | ✅ SOLUSDT 显示 `0*` 并打印"被低估"脚注 |
| 已平仓行 | 不进汇总表, 但**如实报出漏了几个**, `--detail` 可见 | ✅ "另有 1 个组合已无敞口…"; `--detail` 里 `flat_one` 在列 |
| `--pair` / `--mode` 过滤 | 过滤后扫描条数与表同步收窄 | ✅ `--pair ethusdt --mode live` → 2 条持仓 / 4 条订单 / 1 行 / 2 笔挂单 |
| `--mode` 拼错 | **报错**, 不给静默空表 | ✅ exit 1 + `--mode 仅支持 live|demo|dry_run, 收到 'Live'` |
| 空库 / 只剩已平仓行 | 两个空态文案可区分, 都给出下一步 | ✅ "无持仓、无挂单 (本地库 positions/orders 为空)" vs "当前无敞口: … 但头寸全为 0" |
| 表格对齐 | CJK 表头不把后续列挤歪 | ✅ 复用显示宽度补位; 单测 `row_pads_by_display_width` 锁住 |

### P0-A / P0-B

两者都是**纯逻辑单测**覆盖(`live::split_owned` / `must_refuse_start` / `orphan_*_message` 6 例; `oms.rs` 11 例),
接线在 `command.rs` 内不引入新替身。真机链路(demo 下单 → 强杀 → 重启接管)需 testnet 凭据与真实挂单, **留给既有 `#[ignore]` 真机纪律**,
不在本轮伪造。

## 三、收敛期修掉的问题

1. **`must_refuse_start` 漏了"枚举失败"分支**: 初版只判"live 且仍有残留"。但 `get_open_orders` **本身失败**时同样无法确认安全前置 ——
   一样不该进实盘。补 `enumeration_ok: bool` 参数, 判定改 `is_live && (!enumeration_ok || !residual.is_empty())`, 并补 `orphan_query_refuse_message` 与对应单测。
2. **`oms::apply_fill` 用错了成交口径**: 初版按**累计**语义取大, 但 `OrderFill.fill_size` 是**本笔增量**(`OrderUpdate.filled_size` 才是累计)。
   两笔 3+7 会被算成 7 而非 10, 状态机永不进 `Filled`。改为增量累加, 并在文档注释里把两个口径的差别写死, 单测改 `test_apply_fill_incremental_until_full`。
3. **策略撤单指令的合成 ack 漏登记**: `OrderAction::CancelPending` 返回的 ack `client_order_id` 为空(语义是"撤本实例全部挂单", 不回传单号),
   `record_ack` 会跳过 → 登记表里那批在途订单收尾仍显示"挂单中"。新增 `mark_all_canceled`, 在 ack 分支特判
   (`client_order_id.is_empty() && status == Cancelled`)调用。
4. **clippy `unused import: display_width`**: 把显示宽度函数提升到 `commands/mod.rs` 后, `backtest.rs` 侧只剩测试用到它 →
   非测试构建即未使用。改成测试模块内 `use crate::commands::display_width`, 生产侧只导 `pad_display`。
5. **CJK 表头把列挤歪**: 用 `{:<N}` 补位时 Rust 按**字符数**而非**显示宽度**算, 中文表头每个字算 1 却占 2 列 →
   表头之后每列持续右移(实测 `净头寸` 与数据错开 5 列)。发现仓内 `backtest.rs` 早有一份 `display_width` / `pad_display`,
   **提升为 `commands/mod.rs` 的唯一实现**(`pad_display` 右补 + 新增 `pad_display_right` 左补), 两处共用。
6. **展示层的诚实性补丁**: ①已无敞口的 `(标的 × 模式)` 组合不进汇总表(答不了"押了多少", 只会把真数字挤散), 但**不静默丢弃** ——
   如实打印漏了几个, `--detail` 可见; ②总名义算不出的行标 `*` 并加脚注"被低估", 不给看起来完整的假数字;
   ③挂单数触到 `--limit` 时打印"更早的挂单可能未计入"(把"我只看了最近 N 条"说成"没有挂单"是误导)。
7. **会话账目归集位置错了 —— 漏掉整个停机清理阶段**(收敛期自查发现, **资金安全级漏报**):
   `outcome.oms = registry.counts()` 初版放在**主循环结束处**, 而停机清理(撤单兜底 / 兜底平仓 / 复查)在它**之后**才跑,
   且清理阶段也会改登记表(撤单成功 → `mark_canceled`; 平仓单 → `record_ack`; 平仓传输失败 → `record_unknown`)。
   后果有两层: 轻的是"刚撤的单被算成仍在挂单"; **重的是兜底平仓单提交失败(仓位可能还在)却不进 `Unknown` 清单、不触发"请立即核对交易所"告警**。
   修法: ①把归集与未知项告警**移到停机清理 + 用户流吸干之后**; ②给 `drain_user_events` 加 `&mut OrderRegistry` 参数 ——
   清理窗口内到达的成交/订单回报同样是本次会话的账, 不喂给登记表就会出现"平仓成交已入库、账目里那单仍显示挂单中"的错位。
   > **为什么没有单测**: 这是**接线/时序**缺陷(与 017 同类), 要覆盖就得引入 Exchange 替身跑完整停机流程 ——
   > 与宪法原则三"交易流程禁 mock Exchange 替身"冲突。处置同 017: **改代码 + 就地注释锁住顺序**,
   > 以真实链路为证(建议在 P4 dogfood 首日一并走"挂单 → 强杀 → 重启接管"闭环, 见 §六 遗留 3)。

## 四、决策记录

- **启动接管不提供 `orphan_policy` 配置**(plan D1): 固定工程不变量, 与 `order_guard` 同口径 ——
  "平台不做投资判断, 但安全不变量不接受配置"。有配置面就必然有"配错了"的失败模式。
- **残留时 live 拒绝启动、demo 只警告**(plan D2): 与既有的**时钟预检**同口径 —— 无法建立安全前置时不进实盘。
  错误信息必须带逐条单号 + 手工撤单指引 + "撤销后重启", 否则用户会被卡住。
- **OMS 落 `ricow_engine`、接线全在 `command.rs`**(plan D4-D5): 不改 `LiveContext` 签名、不给策略任何新可见面(FR-2.5);
  键取**最终** `client_order_id`(已含 `<策略名>-` 归属前缀), 与用户流回报同源, 可精确对账。
- **P0-C 新增 CLI 子命令 `exposure`, 而不是塞进 `status` / `list`**: 026 已确立"既有 CLI 用户可见文案零变化",
  改既有输出会破坏那一条; 且 `status` 答的是"进程状态"、敞口答的是"钱", 两个问题不该混在一张表里。
- **聚合键 = `(标的, 模式)`**(本轮新增约束): dry_run 的持仓是模拟的, 与实盘相加会造出"看似精确定则错误"的总敞口。
- **不跨交易对汇总总名义**: 不同交易对报价资产未必一致(USDT / USDC / BTC), 相加无意义 —— 宁缺勿错, 由展示层逐行给出。
- **验证方式 = 纯逻辑单测 + 手工灌库**, 不引 Exchange 替身: 比仓内既有的 `FundingStubExchange` / `ReconcileStubExchange` 先例更保守(宪法原则三)。
- **P0-D 不做**: 单笔名义上限 / 异常速率熔断必须是**可配置**的才有意义, 而"新增可配置风险参数面"与宪法
  D15/D17(019 R5 删除 `RiskEngine` 四条静态限额、只留固定 `order_guard`, 理由 = 平台不做投资判断)**直接冲突** ——
  属宪法级决策, 不能由实现者在变更里顺手绕开。已写入 roadmap"下一步"第 8 项并给出三个候选方案(维持现状 /
  固定比例常量 / 显式修订宪法), 待用户拍板。

## 五、改动的文件

**新增**:

| 文件 | 内容 |
|:--|:--|
| `crates/ricow_engine/src/oms.rs` | P0-B: `OrderState` / `OrderEntry` / `UnknownSubmission` / `RecordVerdict` / `OmsCounts` / `OrderRegistry`(纯内存, 11 例单测) |
| `crates/ricow_engine/src/exposure.rs` | P0-C: `ExposureFilter` / `StrategyExposure` / `PairExposure` / `ExposureView` / `aggregate[_with]` / `is_open_status`(纯函数, 13 例单测) |
| `crates/ricow/src/commands/exposure.rs` | P0-C 展示层: `ricow exposure` 参数与渲染(12 例单测) |

**修改**:

| 文件 | 改动 |
|:--|:--|
| `crates/ricow_strategy/src/events.rs` | +`KIND_ORPHAN_CANCELED`(不动 schema 版本、不动 `RunEvent` 字段) |
| `crates/ricow_engine/src/live.rs` | +`split_owned` / `must_refuse_start` / `orphan_refuse_message` / `orphan_query_refuse_message`(纯函数, 6 例单测); `plan_cleanup` 内部改用 `split_owned`(零行为变化) |
| `crates/ricow_engine/src/command.rs` | 启动接管接线(替换原"只 warn"块)+ OMS 全链接线(下单 ack / 传输失败入账 / 用户流回报 / 停机清理 / **`drain_user_events` 清理窗口内的成交与回报** / 收尾账目归集在清理之后)+ `RunOutcome.orphan` & `.oms` |
| `crates/ricow_engine/src/lib.rs` | 导出 `oms` / `exposure` 两个新模块 |
| `crates/ricow/src/commands/run.rs` | CLI 收尾打印 orphan 结果 / 订单账目 / unknown 与 duplicates 告警 |
| `crates/ricow/src/commands/mod.rs` | 提升 `display_width` / `pad_display` 为唯一实现, 新增 `pad_display_right`; 挂载 `pub mod exposure` |
| `crates/ricow/src/commands/instances.rs` | 挂单判据改调 `ricow_engine::is_open_status`(唯一实现, 防两视图漂移) |
| `crates/ricow/src/commands/backtest.rs` | 删本地 `display_width` / `pad_display`, 改用 `commands::` 共享实现(**纯搬移, 输出逐字不变**) |
| `crates/ricow/src/main.rs` | 注册 `exposure` 子命令 |
| `crates/ricow/src/ai/prompt.rs` | 命令速查补一句 `ricow exposure`(AI 与 agent-kit 手册同源) |
| `README.md` / `README_zh.md` | "Inspect / 查看状态"一行补 `ricow exposure` |
| `specs/architecture.md` | §五 CLI 命令集 + 用法; §六 新增"组合敞口只读视图(037)"; §七 新增启动接管与引擎级订单登记两条安全口径; §九 基线更新(并订正红线 4 → 5 条) |
| `specs/roadmap.md` | 测试基线(新增 761, 670 降为前基线, 历史链补 736/761) + 变更档案表加 037 行 + 037 变更说明段 + "下一步"第 8 项 P0-D 待拍板 |

## 六、遗留(如实记录)

1. **P0-D 操作级熔断 / 单笔名义上限**: 未做, 待用户拍板(选项见 roadmap"下一步"第 8 项)。
2. **`exposure` 只接了 CLI**: Web 面板与 AI 只读工具尚未透出。AI 侧目前只到"提示词里提到了这条命令"的程度;
   做成结构化只读工具(返回 JSON)需要对 `ExposureView` 加 serde 派生并补一条 Web 端点 —— 属 P1 能力扩展, 本轮不做(YAGNI)。
3. **P0-A/P0-B 的真机取证**: 本轮以纯逻辑单测 + 接线验证为准, 未做"demo 挂单 → 强杀 → 重启接管"的真机闭环
   (需 testnet 凭据与真实挂单)。建议在 P4 dogfood 首日一并走一次, 记入 `specs/testnet.md`。
4. **回测限价单撮合的系统性高估**(报告 §五新发现: "bar 触及即 100% 全成"忽略排队位置, 网格类内置策略回测收益被高估)
   —— 属 🟡 P1, 会改变既有回测数字, 需要单独立项 + 逐位比对, **不在本轮**。
5. `ai_live_smoke` 仍有 9 例 `#[ignore]`(需真实外部环境), 与本轮无关。
