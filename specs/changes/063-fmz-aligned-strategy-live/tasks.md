# 063 任务清单：fmz 对齐的实盘视图 + 策略库改版

> 勾选状态 = 实际进度。**只有真正做完并验过才勾**（档案状态 = 代码状态）。
> 配套：[`spec.md`](./spec.md) / [`plan.md`](./plan.md)

## P0 后端

- [x] **T1** 策略列表补 `last_modified` / `created_at`（`strategies.rs::StrategyRow`）
  - [x] T1.1 用户策略：读 `strategies/<id>.toml` + `strategies/<market>/<id>.lua` 的 mtime，取较新者为 `last_modified`
  - [x] T1.2 `created_at` 取 `.toml` mtime；内置策略两个字段都是 `None`
  - [x] T1.3 读不到 → `None`，**不编造**
  - [x] T1.4 单测：磁盘无文件 → 两字段皆 `None`；用户策略 → 有值
- [x] **T2** 新增 `POST /api/strategies/{id}/instance`（只改参数与交易对，不动代码）
  - [x] T2.1 id 合法 + `catalog::run_block` 目录审计（与其它写端点同口径）
  - [x] T2.2 内置策略 → 409 `builtin`（内置脚本编译期嵌入，改不了）
  - [x] T2.3 读现有实例 TOML → **只改** `params` 与 `pair`；`live_enabled`/`market`/`position_mode`/`backtest` 段原样保留
  - [x] T2.4 `params` 里的 `script`/`script_path` **不覆盖**（引擎装载细节，用户不该改）
  - [x] T2.5 走 `write_strategy_files` 的 `.bak` 备份；落盘后回读校验
  - [x] T2.6 响应回**实际落盘值**（不是回显请求）
  - [x] T2.7 单测：未提及字段没丢 / 内置 409 / `script_path` 不被覆盖
- [x] **T3** 路由注册 + 文档注释（`web/mod.rs`）

## P1 策略库混排表格

- [x] **T4** `strategies.js` 列表改单表（`renderList` 去掉四组分区）
  - [x] T4.1 一张表混排全部策略；内置/我的用**行内徽章**区分（事实，不是分区）
  - [x] T4.2 列：名称 / 最后修改 / 创建日期 / 操作项
  - [x] T4.3 时间读不到 → 显示 `—`
  - [x] T4.4 操作项是**行内文字链接**：编辑 / 复制 / 删除 / 更多
  - [x] T4.5 保留：展开（说明摘要）、诚实性徽章、`sg-firststep` 引导（移到表格下方）
- [x] **T5** 内置策略的「编辑」= 只读详情 + 「复制为我的策略」按钮（**不静默复制**）

## P2 编辑页双 Tab + AI 助手右置

- [x] **T6** 详情页拆双 Tab：「策略编辑」/「模拟回测」
  - [x] T6.1 代码/参数/清单 → 策略编辑 Tab；回测/寻优/样本外/历史 → 模拟回测 Tab
  - [x] T6.2 切 Tab **不销毁 DOM**（只隐藏），避免丢未保存状态
  - [x] T6.3 053 参数未保存提示、059 离开守卫的**锚点 ID 一个字不改**，只挪位置
- [x] **T7** AI 助手移到右侧常驻栏（fmz 形态）
  - [x] T7.1 窄屏（<1200px）降级为 Tab 内联卡片
  - [x] T7.2 fmz 式**快捷指令按钮** 6 条：写一个均线交易策略 / 写一个带止损止盈的网格策略 / 分析我的代码有什么问题 / 给代码添加中文注释 / 写一个 RSI 超买超卖策略 / 写一个双均线交叉策略
  - [x] T7.3 快捷指令**只预填文本，不自动执行**
- [ ] **T8** 「模拟回测」Tab 接 AI 解读（接 `ai-assess`，形成"跑回测 → AI 解读"闭环） —— **本轮未做**：回测整块搬进了 Tab，但「AI 解读」按钮没接进去（端点已存在，属需求 4 的最后半步，见 converge §6）
  - [x] T8.1 初始资金沿用 058 口径：真值 = 实例 TOML `[backtest].initial_cash`；**读不到就不渲染该字段**（既有实现，本轮未动）

## P3 实盘视图 + 新建实盘表单

- [x] **T9** `#runs` 改 fmz 式实盘列表
  - [x] T9.1 列名对齐：名称 / 状态 / **盈利** / **策略** / 操作
  - [x] T9.2 **保留** ricow 独有列：模式 / 成交数 / 运行时长（对齐 fmz 不等于丢事实）
  - [x] T9.3 不做「托管者」「创建日期」两列（无对应事实，不造）
- [x] **T10** 新建实盘表单（替换现弹窗），字段序对齐 fmz
  - [x] T10.1 实盘名称（缺省 = 策略名）
  - [x] T10.2 运行策略：**可搜索**下拉，内置+我的混排
  - [x] T10.3 策略参数表单（类型/默认值/说明/取值框；bool 用开关）—— 复用详情页那套渲染，**不新造一套**
  - [x] T10.4 交易对
  - [x] T10.5 模式：模拟 / 模拟盘 / 实盘
  - [x] T10.6 实盘时：逐字短语 + 风险披露 —— **沿用现门禁，一字不改**
  - [x] T10.7 选内置策略 → 提示「将先复制为你的策略」→ 走复制路径
  - [x] T10.8 按钮叫「创建并启动」（ricow 无"已建未跑"中间态）
  - [x] T10.9 落盘走 `POST /{id}/instance`，启动走 `POST /{id}/start`
  - [x] T10.10 资金不足：**如实透出**引擎的 `insufficient balance`，前端不拦

## P4 验证与文档

- [x] **T11** 门禁六件全绿
  - [x] T11.1 `cargo fmt --all -- --check` → 0 差异
  - [x] T11.2 `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
  - [x] T11.3 `cargo test --workspace --no-fail-fast` → 0 failed
  - [x] T11.4 `bash scripts/ci_grep_gates.sh` → 五条红线全绿
  - [x] T11.5 `node scripts/web_frontend_check.mjs` → 全绿
  - [x] T11.6 真机两路：e2e / CDP **与基线逐条断言名一致**（双向 `comm` 为空）
- [x] **T12** 063 自带走查（**新 root + 全新浏览器 profile** —— 062 的死连接坑）
  - [x] T12.1 实盘列表列名 / 新建实盘全流程 / 策略库混排表 / 双 Tab / AI 快捷指令
- [x] **T13** 变异验证：新断言逐条注入变异，**全被抓住**
- [x] **T14** 文档：`converge.md` / `roadmap.md` 加 063 行 / `architecture.md` 第九节同步 / 基线数字更新
- [x] **T15** 项目记忆：把本轮踩的坑写进 `.workbuddy/memory/`
