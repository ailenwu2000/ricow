# 002 功能规格: AI 量化研究员入口 (建策略闭环 CLI + Dry Run 时长门禁)

**状态**: 未开始 | **日期**: 2026-09-13 | **依据**: `specs/roadmap.md`(002 backlog)、`specs/architecture.md` §七/§十一 A、`specs/lua-api.md` 开头、`specs/product.md` §三.2/§三.3

## 一、为什么

`architecture.md` §七 记录的**写操作强制确认链路**(提交 → 编译门禁 → 沙箱回测 → preview → 批准 → 部署)在引擎侧**已全部实现**:
`locus_engine::create_strategy`(编译门禁)、`Engine::create_strategy_preview`(门禁 → 拉 K 线 → 回测 → preview)、
`locus_engine::confirm::{create_preview, approve, consume}`(两步确认)、`locus approve`(用户批准)。

**但没有任何 CLI 命令把这条链路暴露出来** —— 于是:
- AI 客户端(读 `lua-api.md` 写 Lua 的那个角色)无法把生成的策略交进来;用户只能手写 `strategies/<name>.toml` + `.lua`;
- `architecture.md` §十一 A 记着另一个缺口: `dry_run_started_at` 字段**无生产方也无消费方**(谁写、谁读都没定义),而 product.md 的"默认 Dry Run 观察 → 确认后切实盘"缺一道可核验的时长条件。

本变更把这两件事一起收口: **给 AI/用户一条可走的建策略路径, 并让"先 Dry Run 观察"从口头约定变成可核验的门禁。**

## 二、范围

**做**: ① CLI 建策略闭环(`create` → `approve`(已有) → `deploy`; 部署物 = `strategies/<name>.toml` + `<name>.lua`);
② `Engine::execute_strategy`(部署侧写操作, 走一次性 token 重放); ③ `dry_run_started_at` 的写入(Dry Run 首次启动)与读取(实盘启动时长门禁)。

**不做**: 不内置 LLM(架构既定: AI 智能在外侧客户端); 不引入新入口形态(MCP/TG/Web); 不改动写操作确认的安全模型。

## 三、用户故事

- **US1(AI 客户端 / 进阶用户)**: 我生成了一段 Lua 策略, 想先让 Locus 校验它、拿真实 K 线沙箱回测一次, 拿到报告后**再**决定是否落盘部署 —— 中间不能一步就把文件写进我的策略目录。
- **US2(用户)**: 我批准后, 部署产物应该是我能直接 `locus run <name>` / `locus start <name>` 的东西, 而不是只有我能看懂的内部记录。
- **US3(风控)**: 我声明了实盘并加了 `--live`, 但如果这个策略从没真正在 Dry Run 下跑过足够久, 我不想它在第一次启动就去碰真钱。

## 四、功能需求

- **FR-001 提交即门禁**: `locus create` 接收 Lua 代码(文件 / stdin / 含围栏的 AI 响应全文), 走 `create_strategy` 编译门禁; 门禁失败**不落任何文件、不产生 preview**。
- **FR-002 沙箱回测前置**: 门禁通过后按真实交易所 K 线沙箱回测, 报告先打印给人看; 回测失败(K 线为空/参数非法)不产生 preview。
- **FR-003 两步确认不可绕过**: 部署只认 `(preview_id, token)`; `token` 只能由 `locus approve` 从 `pending` 状态生成, preview 有 15 分钟 TTL 且一次性消费。
- **FR-004 部署产物可读可跑**: 落盘 `strategies/<name>.toml`(`params.script_path = "<name>.lua"`)+ `strategies/<name>.lua`;部署后 `locus run <name>` 必须能直接加载(CJK: 与既有 loader 口径一致)。
- **FR-005 幂等与不覆盖**: 已有同名策略文件时**默认拒绝**部署并提示(避免覆盖用户已部署的策略); 不静默改名。
- **FR-006 Dry Run 起点留痕**: 策略以 Dry Run 模式启动时, 若 `dry_run_started_at` 缺失则写入并落盘该策略 TOML(已存在则不改写)。
- **FR-007 实盘时长门禁**: 实盘启动时, 若 `dry_run_started_at` 存在且 Dry Run 累计时长 < `min_dry_run_hours`(默认 24 小时, 可用该参数设 0 关闭) → **拒绝启动**并打印已运行时长/阈值/关闭方法; `dry_run_started_at` 缺失同样拒绝(从未 Dry Run 过)。
  依据: Dry Run 的意义是让策略在真实行情下暴露错误, 24 小时 = 覆盖亚/欧/美三个交易时段的一个完整日周期; 该值可由用户按策略节奏下调或关闭, 不做"拍脑袋保守放大"。
- **FR-008 拒绝要可诊断**: 门禁与时长不足的拒绝消息都包含: 当前状态(已运行多久 / 从未运行)、阈值、以及**具体的解除方式**(设 `params.min_dry_run_hours`)。

## 五、验收标准

- **SC-001**: `cargo test --workspace` 全绿且无回归。
- **SC-002**: 纯逻辑单测覆盖: 部署产物生成(文件名/内容/不覆盖)、时长门禁判定(缺失/不足/刚好/关闭)、preview token 不能被跳过。
- **SC-003**: 真实链路: 提交一段 Lua → CLI 门禁 → 真实 K 线沙箱回测出报告 → preview → `approve` → `deploy` → 部署物存在; 之后 `locus run <name>`(Dry Run)能真正启动并 tick。
- **SC-004**: 门禁负例真实可复现: 语法错误代码被拒且**无任何文件落盘**; 已存在同名策略时 deploy 拒绝。
- **SC-005**: 时长门禁负例真实可复现: `live_enabled=true` + `--live` + 刚写的 `dry_run_started_at` → 拒绝启动并给出解除方式(不产生任何真实订单)。
- **SC-006**: 文档同步: `architecture.md`(§五 CLI 命令集 / §七 现状注记删除 / §十一 A 关闭 / §九 基线)、`lua-api.md`(部署路径不再是"仅手写")、`roadmap.md`(002 行)、`product.md`(§三.3 入口落地)、本档案 converge。

## 六、假设

- **A1**: 沙箱回测的默认周期沿用现有实现(90 天 1h); 需要别的周期时用户走 `locus backtest`。
- **A2**: 部署目录 = `LOCUS_ROOT/strategies/`(现有 loader 读同一目录)。
- **A3**: 时长门禁只在**实盘**启动路径生效; Dry Run/回测不受影响(否则会挡住正常使用)。
