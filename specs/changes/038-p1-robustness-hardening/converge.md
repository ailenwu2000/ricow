# 038 收敛记录 (P1 工程健壮性加固)

**功能目录**: `specs/changes/038-p1-robustness-hardening`

**收敛日期**: 2026-10-06

**依据**: [`specs/research/framework-vs-commercial-2026-10.md`](../../research/framework-vs-commercial-2026-10.md) §四 🟡 P1 —— 本轮把 **P1-A ~ P1-F 六项全部实施**; 🔴 **P0-D 仍显式不做**(需可配置风险参数面, 与宪法 D15/D17 冲突, 待用户拍板)。

**用户指令**: "再次深入修复下P1级bug,提高工程健壮级"

**范围与口径(用户两轮澄清后选定)**:

| 项 | 用户选定 |
|:--|:--|
| 范围 | **全部六项**(P1-A ~ P1-F) |
| P1-A 默认 | **默认不变 + 暴露手段** —— 新增穿透参数默认 `0`(既有回测数字与全部断言零变化), 报告里单列"限价单成交笔数/占比"与乐观假设提示, 用户可主动收紧 |

## 一、实跑门禁(2026-10-06, Windows, agent 环境)

| 门禁 | 结果 |
|:--|:--|
| `cargo fmt --all -- --check` | **0 差异** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0、零代码告警**(残余 warning 全为 Windows 增量编译锁文件 `os error 5` 环境噪声, 与既有基线记录一致) |
| `cargo deny --locked check` | 全绿(`advisories ok, bans ok, licenses ok, sources ok`) |
| `bash scripts/ci_grep_gates.sh` | **5 条安全红线全绿**(AI 工具层零落盘 / 明文密钥不进日志 / 无调试残留 / `execute_strategy` 调用点白名单 / `Instant` 无裸减法) |
| `cargo test --workspace --no-fail-fast` | **806 passed / 0 failed / 22 ignored** |
| `architecture_guard` | **3 / 3 全绿**(含规则 A 引擎+绑定层、规则 B CLI 层、清单↔Lua 键一致性) |

> 上表为**最终**实跑值(含 §三 第 10~13 条同源缺陷修复带来的 4 例增量; 修复前的中间态是 802)。

**测试增量 = +45**(与本变更一一对应, 其余 target 一字未变):

| target | 前 | 后 | 增量来源 |
|:--|--:|--:|:--|
| `ricow` bin | 382 | **407** | +25: `commands/align.rs` 汇总/窗口还原/渲染/诚实性文案 8 + `supervisor/server.rs` 重启判定/退避/配置读取/停机收摊 6 + `web/metrics.rs` 渲染/转义/空骨架/token 门 6 + `supervisor/procs.rs` 子进程 stdin 停机链路的跨平台覆盖 4 + 其余 1 |
| `ricow_strategy` lib | 184 | **197** | +13: `gaps.rs` 缺口检测 8 + `backtest.rs` 限价穿透 5 |
| `ricow_engine` lib | 112 | **119** | +7: `latency.rs` 延迟分位数 |
| ricow_binance 59 · ricow_core 18 · `architecture_guard` 3 · `ai_live_smoke` 3 | — | — | 未变 |

**不变量核对**(spec §不变量, 违反即回滚):

| 不变量 | 结论 |
|:--|:--|
| 架构铁律: 引擎/CLI/绑定层零策略参数名 | ✅ `architecture_guard` 3/3; 新增代码只用 `pair` / `mode` / `status` / `state` 这类通用键与结构字段名。`limit_fill_penetration_bps` 与既有 `slippage_bps` / `fee_maker_bps` 同属**回测模型参数**, 不是策略参数 |
| 既有回测数字零变化 | ✅ 穿透默认 `0` 时 `try_match_ohlc` 走原分支(`limit` 变量取原值), 成交价仍是 `req.price`; 全部既有回测断言未改一字 |
| 既有 CLI 用户可见文案零变化 | ✅ 新能力一律新增子命令(`align`)/ 新参数(`--limit-fill-penetration-bps`)/ 新端点(`/metrics`); 既有命令的既有输出只有**新增行**(回测报告多一行"限价单成交") |
| 写确认面不变 | ✅ 本轮新增的全部是**只读**能力(`align` / `/metrics`)与**运维开关**(`[supervisor]`); 无新写路径、无新确认渠道 |
| 禁 mock Exchange 替身 | ✅ 未引入任何 Exchange 替身; 端到端只跑真二进制 + 真 SQLite + 真回环 HTTP, 不连 testnet |

## 二、端到端模拟验证(用户授权: "相关问题可以模拟验证")

**方式**: 临时数据目录(`RICOW_ROOT=tmp/e2e-038`)+ Python `sqlite3` 手工灌 `fills` / `orders` / `positions` / `pnl_snapshots` + 手写一张 run card, 再跑**真实二进制**。
**脚本**: `tmp/e2e_038.py`(可重复跑, 已验证幂等)。

### P1-E `ricow align`

| 场景 | 观察 |
|:--|:--|
| 默认(`--mode live`) | 窗口由 run card 还原为 `2026-10-05 00:00:00 UTC → 2026-10-06 00:00:00 UTC`; 库内 12 条 → **命中 7 条**(正确排除 3 条 demo、1 条窗口外、1 条他 pair); 表内"成交笔数 120 / 7 / −113"; 口径说明四条全部印出 |
| `--mode demo` | 命中 3 条, "实盘侧: mode=demo" 如实标注 |
| `--card <不存在的路径>` | 非 0 退出, 错误**点名那个路径** |
| 无 run card 的策略 | 非 0 退出, 错误给出下一步与已查看目录 |
| `--mode bogus` | 非 0 退出, 错误列出合法取值 `live / demo / dry_run` |

### P1-B `GET /metrics`

| 场景 | 观察 |
|:--|:--|
| 无 token | `401` 且**响应体为空**, 不含任何指标名 |
| 带对 token | `200`; 7 类指标 HELP + **TYPE** 齐备; `ricow_open_orders{strategy="grid_e2e"} 1`(另两条 filled 不计)、`ricow_position_size{...} 1.5`、`ricow_net_pnl{...} -12.34`(Decimal 原样、不加引号)、`ricow_fills_total 12`; 每条样本行均以数值收尾 |

### P1-C `[supervisor]`

| 场景 | 观察 |
|:--|:--|
| `restart_policy = "on-failure"` | daemon 启动日志打出 `INFO supervisor: 已启用崩溃自动重启 (on-failure): 主动停机不会触发, 重启前会走启动挂单接管 max_retries=3 backoff_secs=5` —— 配置 → 运行态接线成立 |
| `restart_policy = "bogus"` | daemon **照常启动**(不因配置写坏而起不来), 打出 `WARN ... 仅接受 "none" / "on-failure", 实际为 "bogus"` 并按"不重启"继续 |
| 生成的 `ricow.toml` 模板 | ⑤ 段落入 `[supervisor] restart_policy = "none"`, 原密钥环段顺延为 ⑥ |

## 三、收敛期修掉的问题(全部在实现/验证期暴露)

| # | 问题 | 性质 | 处置 |
|:--|:--|:--|:--|
| 1 | `config_file::check_keys` 的类型表漏了 `[supervisor]` 的 `max_retries` / `backoff_secs` | **真 bug**(用户可见) | 用户照模板写 `max_retries = 3` 会被回一句"必须是字符串" —— 看着像配置写错, 其实是校验写错。补两个整数键, 并把 `ai.max_turns` 同处的措辞一并订正为"整数" |
| 2 | `/metrics` 的 `ricow_daemon_up` 只有 HELP 没有 TYPE | 格式缺陷 | Prometheus 侧解析不完整。补 `# TYPE ricow_daemon_up gauge`(此前只有它一处漏) |
| 3 | 标签转义断言写错: `!line.contains("x\"")` | 测试缺陷(假警报) | 转义后的 `\n` 后面紧跟用户数据的 `x`, 再接标签收尾引号, 那个 `x"` 是**合法输出**。改为"样本行只有一条且以值收尾" |
| 4 | 停机收摊测试的替身子进程只在 `#[cfg(unix)]` 分支创建 | 跨平台测试缺陷 | macOS/ubuntu 过、**windows-latest 上 `graceful` 恒为 0 → 必然红**。改成两平台各起一份(`sh -c exit 0` / `cmd /C exit 0`)。**这条其实是两次独立的失败**, 别混成一件事: ① 平台门控(上句); ② 加 Windows 分支后最初写成 `stdin(Stdio::piped())`, 在本机 agent 进程树内 `spawn` 直接失败 `ERROR_PIPE_BUSY(231)`。最终 stdin 给 `null` —— 本测试要验的是 `stop_all` 的 drain + `wait_exit`, 本就不该依赖"往子进程 stdin 写停机指令"那条链路(那条由 `procs::request_stop` 自己的单测覆盖) |
| 5 | 卖侧穿透用例失败(默认口径下也没成交) | 用例设计错误 | 现货卖单**无仓可平**时在 `is_noop_fill` 那层就被挂成 pending, 根本走不到穿透判定 —— 那样测的不是穿透而是 L4。改成先市价建仓再挂卖单 |
| 6 | `cargo fmt` 报 9 文件 diff; `backtest.rs` 的 `} = args;` 与下一行黏在一行 | 我编辑期的机械损伤 | `cargo fmt --all` 修好(纯格式, 不改语义) |
| 7 | `align.rs` 里 `project_root()` 误加了 `?` | 编译错 | `project_root()` 直接返回 `PathBuf`, 不是 `Result` |
| 8 | `render_report` 8 个参数触发 `clippy::too_many_arguments` | lint | 把实盘侧四项(`mode` / `scanned` / `matched` / `facts`)打包成 `LiveSide` —— 顺带修掉"分开传容易在调用点串错"的隐患 |
| 9 | 第 4 条②的 231 被写成"本地沙箱拦匿名管道", 且代码用 `.spawn().ok()` + `.expect("起一个立刻退出的替身进程")` | **归因误判 + 反诊断写法** | `.ok()` 把真实 errno 吞了, 事后只能靠同旧记录类比去猜 —— 这正是误判的入口。2026-10-06 用**零 ricow 代码**的判别性探针(`pipe_probe2.rs` / `pipe_probe3.rs` / `mod_probe.rs`)复核后订正: ① **"沙箱"方向没错, 错的是"关沙箱无效 ⇒ 不是沙箱"这个推论** —— 策略开关并不会卸载已注入的 hook DLL; 用 `mod_probe.rs` 枚举本进程模块, **非 System32 的只有 agent 自带的沙箱 DLL `tsbx.dll`、没有任何 360 模块** ⇒ 注入者即 `tsbx.dll`(360 排除); ② **不是"匿名管道被拦"** —— 同一进程里 `std::io::pipe()`(Win32 `CreatePipe`)拿读端当子进程 stdin **完全正常, 子进程真收到写入的字节(退出码 0)**, 被拦的只有 Rust std 为子进程 stdin 走的那条 NT 具名管道路径; ③ **与版本无关**(rustc 1.83 / 1.85 / 1.96.1 三版行为一致); ④ 排查掉"第一次 spawn / 偶发"(`stdin=null` 先跑 OK、紧随的 piped 必 231、连做 3 次全 231)。代码改为 `unwrap_or_else(\|e\| panic!("... {e} (raw_os_error={:?})", e.raw_os_error()))`, 源码注释 / `roadmap.md` / `architecture.md` / `ai_live_smoke.rs` / 复核 skill 一并订正 |

| 10 | `supervisor::procs` 的 `stop_instruction_and_wait_exit` / `wait_exit_times_out_without_exit` **也是** `#[cfg(unix)]` | **同源覆盖缺口(Windows 零覆盖)** | 承接第 9 条复核顺带挖出的同类病: 这两条要 `sh`, 于是「停机指令 → 优雅退出」这条链在 Windows 上**一条断言都不跑**, 而 CI 是三平台矩阵 —— 与第 4 条同一个形状, 只是这次躺在测试文件里没人看。改法: ① 平台分支写 `#[cfg(not(windows))]` 而非 `#[cfg(unix)]`(门开得更宽, 不会再出现某平台被悄悄排除); ② 替身两平台各一份(Windows 用 `cmd /V:ON` 的 `set /p`, 读一行即返回); ③ 子进程 stdin 走 `std::io::pipe()`; ④ 超时用例改为"让子进程**阻塞在 stdin 上**"(Windows 没有干净的单进程 `sleep`, 不该为此依赖平台计时命令) |
| 11 | Windows 替身最初想用 `findstr`, 后又试链式 `if not '!L!'=='stop' if not '!L!'=='stop --close-all'` | 设计错误(我自己的) | `findstr` 要读到 **EOF** 才退出 —— 与"生产靠子进程**读到那一行就退**"(daemon 之后才关 stdin)不同构, 测的根本不是同一件事。**这条是被探针拦下的**: 把写端全程不关, `findstr` 版本 `wait_exit` 直接超时; 换 `set /p` 后同条件通过。链式 `if not` 则在操作数含空格时被 cmd 按空格切词(实测报 `'--close-all'' 不是内部或外部命令` 并返回 1)。最终只比较**单 token** 的 `stop`, 带平仓意图的形态改由 `write_stop` 直测覆盖(它本来更该在那一层测) |
| 12 | `request_stop` 的 happy path 无跨平台覆盖; `write_stop` 的 **flush** 与**错误冒泡**无任何覆盖 | 覆盖缺口 | 生产侧抽出 `write_stop<W: Write>` 一行缝(`request_stop` 公开签名不变、生产调用点不动), 使"指令原文 + **flush 必被调用**(漏了它停机指令会卡在缓冲里直到 `STOP_WAIT` 超时) + 写失败原样冒泡"可用普通 `Write` 直测。生产路线用例 `request_stop_writes_through_real_child_stdin` 用**三道闸精确跳过**: 只认 231 这一个 errno / 跳过前做正对照(同进程 `std::io::pipe()` 必须可用) / 明确打印。**这道闸刻意做窄** —— 只为"已由探针证实的已知环境行为"放行, 其它任何失败(命令不存在、权限…)照样让测试红, 否则它就是掩盖缺陷的借口 |
| 13 | `ai_live_smoke` 两个门禁用例拿**文件句柄**当 stdin | 名实不符的绕行 | 用例名写着 `piped_*`, 夹具却是临时文件 —— 是第 9 条那次误判留下的产物。已换回 `std::io::pipe()` 造的**真管道**(同一 `CreatePipe` 路线, 不受那条 NT 具名管道拦截影响), 临时夹具文件与其两处清理代码一并删除 |

## 四、决策记录(与 plan.md 一一对应)

| 决策 | 选择 | 理由 |
|:--|:--|:--|
| P1-A 默认值 | `0`(**不是**调研报告建议的"保守值") | 悄悄改默认会让既有回测数字与全部断言**一次性失效**, 而用户无从得知差异来自哪里。改为"默认不变 + 报告把假设顶到眼前" |
| P1-A 成交价 | 保持**原始限价** | 穿透只用于**判定**; 在无 tick 数据时伪造额外滑点是另一种失真 |
| P1-B 实现 | 手写暴露格式, **不引 `prometheus` crate** | 格式就是 `name{label="v"} value\n`; 为它引依赖违反宪法"少而精" |
| P1-B 端口 | **复用既有 web 服务**, 不新开端口 | 新端口 = 新攻击面, 且要再解决一次 token 门 |
| P1-B 不暴露项 | 对账修正次数 | 该量只在 `RunOutcome` 内存里、**未持久化** —— 暴露它等于给一个假数字(调研报告原建议里有这一项, 此处**明确不做**) |
| P1-C 默认 | `none` = 与加这段之前一字不差 | 唯一能"在无人看屏幕时自动拉起实盘进程"的开关, 默认必须关 |
| P1-C 配置写坏 | **硬拒 + 降级为不重启**, 不阻断 daemon 启动 | 安全方向: 少做一次动作; 且 daemon 不该因一份写坏的配置文件起不来 |
| P1-C 重启路径 | 走既有 `Server::start` | **不绕过** 037 定的启动挂单接管(否则"重启 = 敞口翻倍")与实盘三判据 |
| P1-C 不进 Web 可写面 | `SupervisorSection` 只进 `load` 路径 | 与 `ai.allow_custom_base_url` 同理: 改文件即人工确认 |
| P1-D 阈值 | **不设阈值**, 有缺口即硬报错 | "超阈值才报"会让人以为小缺口是数据本身如此; 与既有"预热不足硬报错"同口径 |
| P1-D 落地层 | 绑定层纯函数 `ricow_strategy::gaps.rs` | 引擎不碰"数据完整性"这类编排职责; 且纯函数好测 |
| P1-E 形态 | **独立只读子命令** `ricow align` | 塞进回测报告的附加段会让"回测"与"对照实盘"两件事互相拖累(对照需要实盘数据存在) |
| P1-E 诚实性 | 三条硬约束(口径显式 / 样本不足直说 / 现金流不叫盈亏) | 违反其一即等于给出误导结论 |
| P1-F 落点 | 只写 `RunOutcome` | **不动 `RunEvent` schema**(037 已冻结), 不新增字段、不改 `EVENTS_SCHEMA_VERSION` |
| P1-F 分位数 | nearest-rank 纯函数, **不碰 `Instant`** | CI 红线 5: Windows 单调时钟锚在开机时刻, 对 `Instant` 做裸减法会 panic |

## 五、改动的文件

**新增(4)**

| 文件 | 内容 |
|:--|:--|
| `crates/ricow_strategy/src/gaps.rs` | P1-D 缺口检测(纯函数 + 8 单测) |
| `crates/ricow_engine/src/latency.rs` | P1-F 延迟分位数(纯函数 + 7 单测) |
| `crates/ricow/src/web/metrics.rs` | P1-B `/metrics`(渲染 + 采集 + 6 单测) |
| `crates/ricow/src/commands/align.rs` | P1-E `ricow align`(汇总 + 渲染 + 入口 + 8 单测) |

**修改(17)**

| 文件 | 改动 |
|:--|:--|
| `ricow_strategy/src/config.rs` | P1-A: `BacktestToml` / `BacktestParams` 加 `limit_fill_penetration_bps`(默认 0) |
| `ricow_strategy/src/backtest.rs` | P1-A: 穿透判定 + 限价/市价分列计数 + 报告字段; 5 个新单测 |
| `ricow_strategy/src/lib.rs` | 导出 `gaps` 模块 |
| `ricow_engine/src/command.rs` | P1-F: 三处下单点计时, 收尾写 `RunOutcome.order_latency` |
| `ricow_engine/src/lib.rs` | 导出 `latency` |
| `ricow/src/commands/mod.rs` | P1-A: 报告新增"限价单成交"行; 注册 `align` 模块 |
| `ricow/src/commands/backtest.rs` | P1-A CLI 参数 + P1-D 取数后校验 + run card `Deserialize` / `read_run_cards` / `is_run_card_file` |
| `ricow/src/commands/new.rs` | `BacktestToml` 构造补新字段 |
| `ricow/src/commands/config_file.rs` | P1-C: `[supervisor]` 段 + 校验 + 模板 + `SECTION_LIST` + `int_opt`; 修 `check_keys` 类型表 |
| `ricow/src/commands/run.rs` | P1-F: 收尾打印延迟分位数 |
| `ricow/src/supervisor/server.rs` | P1-C: 策略/重试计数 + `restart_decision` / `restart_backoff` / `load_restart_policy` / `handle_child_exit` + 6 单测 |
| `ricow/src/supervisor/procs.rs` | 抽出 `write_stop<W: Write>` 一行缝(生产签名与调用点不变), 使"指令原文 / flush 必被调用 / 写失败原样冒泡"可直测; 停机链路用例改两平台原生替身(`#[cfg(not(windows))]`), 生产路线用例加三道闸精确跳过; 4 个新/重写单测 |
| `ricow/src/web/mod.rs` | P1-B: 挂 `.merge(metrics::routes())` |
| `ricow/src/web/backtest_jobs.rs` | `BacktestRunSpec` 补字段; `JobStore::counts`(供 `/metrics`) |
| `ricow/src/web/terms.rs` | P1-A: 报告术语表加"限价单成交"(zh/en) |
| `ricow/tests/ai_live_smoke.rs` | 两个 stdin 门禁用例从**文件句柄**换回 `std::io::pipe()` 真管道(与用例名 "piped" 名实相符), 临时夹具文件与清理代码一并删除 |
| `ricow/src/main.rs` | 注册 `ricow align`(use / enum / dispatch) |

**文档(7)**: `specs/architecture.md`(§五 命令集与 `align` 条目、§六 038 小节与配置段清单、§七 安全模型两条、§九 基线 806)/ `specs/roadmap.md`(基线 + 历史链 + 变更档案行 + 详述段)/ `specs/research/framework-vs-commercial-2026-10.md`(P1 实施状态)/ `specs/research/strategy-gate-coverage-audit-2026-10.md`(审计前提"daemon 不 respawn"随 P1-C 失效的时效性标注)/ `README.md` / `README_zh.md` / `crates/ricow/src/ai/prompt.rs`(命令速查 + 易跑偏点第 12/13 条)。

## 六、遗留(如实记录)

| 项 | 状态 |
|:--|:--|
| **P0-D 单笔名义上限 / 异常速率熔断** | **仍未做** —— 需可配置风险参数面, 与宪法 D15/D17(删 `RiskEngine` 四条静态限额、只保留固定 `order_guard`)**直接冲突**, 属宪法级决策, 待用户显式拍板后立项 |
| P1-A 的 tick 级撮合 / 成交概率模型 | 不做(与"不做 tick 级回测"的定位冲突) |
| P1-C per-instance 重启策略 | 不做(YAGNI, daemon 级足够) |
| P1-B 的 tick 速率 / 拒单数指标 | 不做(瞬时速率需窗口状态, 而 `/metrics` 是**无状态采集**; 拒单数只在 `RunOutcome` 内存里, 与"对账修正次数"同理不可暴露) |
| `align` 是否进 Web UI | 本轮只做 CLI。Web 值: 运行页加一个只读对照面板 —— 属独立变更, 未纳入 |
| 实盘侧确认(非模拟) | `align` 读到的是**本进程落库**的成交; 手工下单 / 其它实例 / 未落库的成交不在内 —— 这一点已在输出里逐字写明, 不做补偿性推断 |
