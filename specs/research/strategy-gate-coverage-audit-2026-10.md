# 策略运行门禁覆盖面与运行态边界 审计 (2026-10-05)

> 调研档案: 结论与证据留档, 已定稿不回改。
> 缘起: 2026-10-05 落地"目录诚实性标记 + 运行门禁"(`catalog::declared` / `duplicate_of` / `run_block`,
> 见 `specs/changes/035-engineering-hardening/` 之后的 Web 侧加固轮次)后, 复核"门禁只挂在启动/回测这一层、
> **已经在跑的实例不受影响**"这一设计选择的边界。**本轮只做审计与记录, 未改任何代码。**

## 一、结论: 运行态不设门禁是**正确**的, 不是遗漏

三条互相独立的证据:

1. **运行期不重读脚本 —— "运行中实例刚变可疑"在时序上不存在。**
   `load_strategy` 只在实例**启动时**调用一次: `ricow_engine/src/command.rs` 的 Dry Run:640 /
   实盘+demo:1019 / 回测:1529; tick 循环内不重新读 `.lua`。策略在实例进程内是编译好的内存对象,
   磁盘上那份文件后续怎么变都与运行中的实例无关。
2. **daemon 不 respawn —— "停→起"必经有人值守的启动入口。**
   `ricow/src/supervisor/server.rs:367` 的子进程监控发现子进程自行退出(崩溃/行情流中断)时
   **只落台账**(供 `list` 如实展示退出码), **不重新拉起**。因此不存在"绕过门禁一次就永久绕过"。
   > ⚠️ **本条前提已于 2026-10-06 随 038 P1-C 失效**(仅当 `ricow.toml` 显式写了 `[supervisor] restart_policy = "on-failure"`;
   > 默认 `none` 时行为与本审计撰写时一字不差)。开启后 daemon 会按 `max_retries` / `backoff_secs` 自动拉起异常退出的子进程。
   > 重启走既有 `Server::start`(与首次启动同一道门), 该函数**不含** `catalog::run_block`; 语义上"恢复一个已准入的实例"是对的,
   > 但与本文 §一 缺口 2(`ctrl::start_daemon` 无闸)**叠加**会放大后果: 未声明副本只要起过一次, 之后崩溃会被反复拉起且全程不过目录门禁。
   > ⇒ 结论不变(闸仍应放在"新起一个实例"这个决策点), 但**收口缺口 2 的必要性上升**。
3. **唯一的"持续门禁"实现手段是热停正在跑的实例 —— 不可接受。**
   要强制一个运行中实例停止, 就得为"文件命名卫生"问题中断一个可能持真实仓位与挂单的实例;
   这与既有 FR-024 纪律(实盘不因改参数自动重启 / 重启须重过三判据 / 拒绝而非静默降级)
   属于同一条原则: **绝不为非交易原因动正在跑的实盘**。

⇒ 门禁放在"新起一个实例"这个**唯一存在的决策点**, 逻辑上是完备的;
把闸设在运行态既无必要, 也有害。

## 二、运行态真正缺的是**可见性**, 不是门禁

- 门禁**消除不了**"已经并列跑着的两份副本"(闸上线前起的、或从 CLI/AI 起的) ——
  这个重复下单敞口只能靠人停。这是"运行态不受影响"最实质的影响面。
- 但 `GET /api/runs` 与 `GET /api/strategies/{id}/status`(`ricow/src/web/runs/mod.rs` 的
  `RunItem` / `StatusReply`)只带 `name` / `running` / `mode` / `pid` / `uptime_secs` / `pair` /
  `source` / `pnl` —— **不含** `declared` / `duplicate_of`。
- ⇒ "用户会自己停止再重启"这一前提**在 UI 上不成立**: 用户看不到任何标记, 没有理由去停。
- **建议(未实施)**: 把 `duplicate_of` 透出到运行态**只读**视图 + 前端实例行警告徽章
  (与 `/api/strategies` 行的 `sg-badge-warn` 同口径), 纯展示、零强制、不触碰 daemon。

## 三、顺带查出: 启动门禁的**覆盖面不全** —— 只盖住了 Web

`catalog::run_block` 在 2026-10-05 的**全部**调用点(四处):

| 调用点 | 位置 | 生效 |
|:--|:--|:--|
| CLI 回测 | `ricow/src/commands/backtest.rs:922` | ✅ |
| Web 回测 | `ricow/src/web/backtest_jobs.rs:367` | ✅ |
| Web 寻优 | `ricow/src/web/backtest_jobs.rs:527` | ✅ |
| Web 启动 | `ricow/src/web/runs/mod.rs:296` | ✅ |

而**启动路径的公共汇聚点 `ctrl::start_daemon`(`ricow/src/commands/ctrl.rs:202`)自身无闸**,
它被 **7 处**调用:

| # | 启动入口 | 位置 | 过闸 |
|:--|:--|:--|:--|
| 1 | CLI `ricow start` | `ctrl.rs:77` | ❌ |
| 2 | CLI `ricow restart`(→ `start`) | `ctrl.rs:294-295` | ❌ |
| 3 | AI 改参数后自动重启(dry_run/demo) | `ai/session.rs:1176` | ❌ |
| 4 | AI 启动 Dry Run | `ai/session.rs:1237` | ❌ |
| 5 | AI 启动测试网(demo) | `ai/session.rs:1290` | ❌ |
| 6 | AI 重启实盘(两条) | `ai/session.rs:1345` / `:1378` | ❌ |
| 7 | Web 启动 | `web/runs/mod.rs:375`(第 296 行前置) | ✅ |

⇒ `commands/backtest.rs:919` 注释宣称的"免得网页不让跑但命令行能跑"对**启动**这条线**尚未成立**:
对话里让 AI "启动 X" 照样能起一个未声明的内置副本。

## 四、若要收口到单点: 顺序陷阱(必须先判后停)

`ctrl::restart` = 先 `stop()`(不平仓)再 `start()`。若门禁只加在 `start_daemon` 里,
`ricow restart <未声明副本>` 会**先把实例停掉、再拒绝启动** → 用户拿到的是一个**被停掉的实例**
(可能还带未平仓敞口), 而不是"重启被拒绝"。

处置: restart 侧必须**先判后停** —— 在 `stop()` 之前调 `run_block(&args.name)`, 拒绝则直接返回、
**零副作用**。这与该函数既有的 `// 必须在 stop 之前读: 停机后台账/daemon 视图已被覆盖`
(读 `prev_mode`)属于同一批"必须在停机前完成的前置判定"。

**备选口径**: restart 干脆不拦(该实例本来就在跑, restart 是恢复原位, 不是引入新身份)。
两者是**绑定关系**, 不允许只做一半:

- 收口到 `start_daemon`(一处盖全) ⇒ **必须**配 restart 先判后停;
- 不收口(维持现状) ⇒ restart 不判, 只拦"新起"。

只把闸塞进 `start_daemon` 而不管 restart, 恰好会造出上面那个"停了没起"的坑。

## 五、未实施(待用户定方案)

以上全部建议(第二节的可见性标记、第三/四节的覆盖面收口)**截至 2026-10-05 均未落码**。
两项可拆为独立小变更:

1. 运行态只读标记(小、无风险, 不动 daemon);
2. 启动闸收口到 `start_daemon` + `ctrl::restart` 先判后停。
