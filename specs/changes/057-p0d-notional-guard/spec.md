# 057 — P0-D 单笔名义上限 + G4 跨实例 kill switch

**功能目录**: `specs/changes/057-p0d-notional-guard`
**立项日期**: 2026-10-18
**来源**: `specs/research/requirements-optimization-plan-2026-10.md` Wave 2（G4 + P0-D）+ roadmap「下一步」第 9 项（已拍板 ⓑ）

---

## 一、P0-D 单笔名义上限

### 需求
`framework-vs-commercial-2026-10.md` §四 🔴 P0-D：商用框架普遍有"单笔名义上限 + 异常速率熔断"作为引擎级最后一层操作护栏；ricow 只有固定 100 单/秒的 `order_guard`，**没有按金额的口径**。

### 拍板（2026-10-18，roadmap 第 9 项）
取 **ⓑ**：固定比例的引擎级上限，**不进配置面**。

### 落码
- `crates/ricow_strategy/src/order_guard.rs`
  - `NOTIONAL_LIMIT_MULTIPLE = 10`（单笔名义 ≤ 权益 × 10）
  - `check_notional(notional, equity, multiple)` **纯函数**
  - `quote_asset_of(pair)` → 报价币（**识别不到回 `None`，不猜**）
  - `guard_check(guard, now, notional, equity)`：频率 → 名义，**顺序刻意**
  - `OrderGuardError::NotionalLimitExceeded`
- `crates/ricow_strategy/src/context.rs`：Live / Dry Run 两处 `guard_reject` 接入，各自加 `equity_quote_balance`

### 三条关键决策

1. **倍数取 10**：内置网格/DCA 单笔名义远低于权益；明显失控的下单（把 1000 当 0.1、漏小数点）是**数量级**错误。10 倍这个"宽但不失守"的量级能拦数量级笔误又不误伤正常单。**它拦不住"策略本身就要重仓"—— 那是仓位政策，由策略自管。**
2. **取不到权益就放行**：护栏拒绝必须有依据。没有权益基准时按假定值拒单，会把**正常下单**误拒（比不拦更坏：策略在合法时凭空停手）。宁可漏拦一次数量级笔误，也不误拒一次正常单。
3. **只取报价余额，不加持仓市值**：持仓市值要按逐仓/全仓与合约标记价算，各市场口径不同，算不准反而误拒。

### 刻意不做（边界措辞必须钉死）
- **不做**最大持仓 / 单日亏损 / 回撤熔断 —— 那三项属**投资判断**，继续由策略用 `ctx:net_pnl()` / `ctx:equity()` 自管（宪法 D15/D17）
- **不做**可配置风险参数面 —— 一旦打开，宪法边界实质失效
- 拒单文案自带边界声明（「仓位/回撤政策请在策略里用 ctx:equity() 实现」），单测锁住

---

## 二、G4 跨实例 kill switch

### 需求
现状只有**单实例** `stop --close-all`。多策略场景下用户没有"一键按住全部"的手段 —— 评审列为 G4。

### 落码
- `crates/ricow/src/supervisor/proto.rs`：`Request::StopAll { close_all }`（线上 `cmd=stop_all`）
- `crates/ricow/src/supervisor/server.rs`：`stop_all_instances()` —— **逐个走 `self.stop`**
- `crates/ricow/src/commands/ctrl.rs`：`KillArgs` + `kill()` + `kill_daemon()` + `kill_report_text()`
- `crates/ricow/src/main.rs`：`ricow kill [--close-all]`
- `crates/ricow/src/web/runs/mod.rs` + `mod.rs`：`POST /api/runs/kill`

### 五条关键决策

1. **逐个走 `self.stop`，不复用 daemon 退出的 `stop_all`**：后者是内部收摊路径（`children` 整体 drain），不对外承诺每实例的清理提示/台账/超时回执；kill switch 是用户主动的紧急动作，每实例都要完整停机语义。
2. **与 `Shutdown` 严格区分**：`Shutdown` 停完连 daemon 一起退（"我要关机"）；`StopAll` 只停策略、**daemon 继续活着** —— 紧急按住后还要能 `list` / `status` 看现场。分派错一条 = 紧急动作把 daemon 也关掉（极糟），故有独立单测。
3. **串行不并发**：停机要等子进程退出（最长 30s/实例）。串行时中途某实例超时，后续仍会被处理；状态始终"逐个确定"，不会有一批悬在中间。
4. **`close_all` 默认 false**：kill switch 第一诉求是"停止开新仓"，平仓是独立的不可逆动作，必须显式要。
5. **两条渠道都要逐字短语**：终端 `确认停止全部`（`--close-all` 时 `确认停止全部并平仓`）；Web 端 `confirm` 字段必须逐字匹配，裸 y/yes 不认。理由：停全部是批量不可逆动作，误触一次会让所有策略静默下线（可能带未平仓敞口），而用户以为它们还在跑。

### 报告口径（027 同一原则）
`stopped == 0` 时**不套"已全部停止"头衔**，只说「当前没有运行中的实例 (无需停止)」—— "本来就没在跑"与"已停止"打架会摧毁用户对紧急通道的信任。任一实例未停成则报告里必须带 `已停止 M/N 个` 计数，不说"全部停止"。

---

## 三、验收

| 项 | 结果 |
|:--|:--|
| `cargo test --workspace` | 见 converge.md |
| 门禁五红 · fmt · clippy | 见 converge.md |
| 新增单测 | P0-D **8 条** + kill 协议 2 条 + kill 报告 2 条 + kill 短语 1 条 = **13 条**（原写"7 条"是漏数了 `guard_check_passes_when_notional_unknown`，2026-10-18 按 `order_guard.rs` 实测条数订正） |

## 四、明确不做

1. **不做**最大持仓 / 单日亏损 / 回撤熔断（投资判断，策略自管）
2. **不做**可配置风险参数面（宪法边界）
3. ~~**不做** kill switch 的前端按钮~~ —— **058 已推翻并落地**（挂运行视图表头；用户全程只走 Web UI 是硬需求）。保留理由：只读红线只管对话视图右栏 `<aside id="panels">`，运行视图不在其内；按钮不进顶栏（运行域动作非全局导航）
4. **不做**把 kill switch 注册为 AI 工具 —— 宪法「写实动作不作工具注册」
5. **不做** daemon 侧的批量强杀（与"不静默强杀"既有纪律冲突）