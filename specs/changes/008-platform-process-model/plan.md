# 008 实施计划: 跨平台策略进程管理

**分支**: `008-platform-process-model` | **日期**: 2026-09-12 | **规格**: [spec.md](spec.md)

## 摘要

新增常驻进程管理器 `locus daemon`(pm2 模型): 持有策略子进程, 通过本机 TCP(127.0.0.1 + 随机端口 + token)接受 CLI 指令, 通过 stdin 管道向子进程下发"优雅停机"; 子进程读到管道 EOF 即自知 daemon 已不在, 自行清理并退出。CLI 命令面补齐 `daemon/list/status/start/stop/info/fills/logs`, 替换现有 systemd 转发。零新增依赖, 三平台一套代码。

## 技术上下文

**语言/版本**: Rust(workspace 声明 1.83, 实际构建用 stable; 本变更不新增依赖, 不受 MSRV 影响)

**主要依赖**: **零新增** —— `std::process`(spawn/try_wait/kill/Stdio 重定向) + `tokio`(TCP server/client) + `serde_json`(行协议) + `uuid`(token), 均已在 workspace

**存储**: SQLite 既有 `fills` 表(补接线) + 文件台账 `run/*.json` + 日志 `logs/<name>.log`

**目标平台**: Linux / macOS / Windows(macOS 无实机, 见 spec §四)

**测试**: 单元测试(协议/台账/状态判定) + testnet 真实调用(`#[ignore]`, 币安 demo, 沿 `crates/locus_binance/tests/` 既有基建)

**约束**: `unsafe_code = "forbid"` —— 平台分支只用 safe API(已实测 `process_group` / `creation_flags` 均 safe); 策略层与 Lua 沙箱不变

**规模**: 新增 supervisor 模块(约 400-500 行) + ctrl.rs 重写 + run 停机接线 + 4 处引擎/核心修复

## 宪法检查

- [x] 原则一(完全本地化): daemon 与 CLI 仅通过本机 TCP 127.0.0.1 通信, 端口随机 + token 校验; 无云端、无遥测
- [x] 原则二(策略层 Lua): 不新增 Rust 策略本体; 停机清理是引擎能力(撤单兜底), 策略侧仍是 Lua `on_stop`
- [x] 原则三(测试纪律): 交易与清理流程用币安 demo 真实调用, 禁 mock 替身
- [x] 原则四(产物一律中文)
- [x] 原则五(少而精): 零新增依赖; 不做服务注册/开机自启/进程扫描/心跳

## 项目结构

```text
specs/changes/008-platform-process-model/
├── spec.md      # 功能规格
├── plan.md      # 本文件
└── tasks.md     # 任务分解
```

### 源码改动

```text
新增
  crates/locus_cli/src/supervisor/mod.rs        # daemon 主体: 实例台账 + 调度 + server 装配
  crates/locus_cli/src/supervisor/proto.rs      # 行协议(JSON 单行 请求/响应) + token 校验
  crates/locus_cli/src/supervisor/procs.rs      # 子进程生命周期: spawn / 管道 / 日志重定向 / 等待 / 终止
  crates/locus_cli/src/supervisor/server.rs     # TCP server(本机绑定) + 指令分发
  crates/locus_cli/src/supervisor/client.rs     # CLI 侧客户端(读 daemon.json 连接)
  crates/locus_cli/src/commands/daemon.rs       # daemon start|stop|status(含自后台化)
  crates/locus_cli/src/commands/instances.rs    # list / status / info / fills

修改
  crates/locus_cli/src/main.rs                  # 子命令注册
  crates/locus_cli/src/commands/ctrl.rs         # 重写: 去 systemctl 转发 → daemon 客户端
  crates/locus_cli/src/commands/run.rs          # 前台保留; 新增 stdin 停机指令监听与优雅退出
  crates/locus_cli/src/commands/logs.rs         # 实装: 读 logs/<name>.log + 轮询尾随
  crates/locus_cli/src/commands/mod.rs          # 数据目录(平台标准目录 + 现状回退)与 run/ logs/ 路径
  crates/locus_engine/src/command.rs            # on_stop 调用 + 停机信号 + 下单错误不吞 + fills 落库
  crates/locus_core/src/exchange.rs             # Exchange trait 增 get_open_orders
  crates/locus_binance/src/spot.rs / futures.rs # openOrders 实现(现货 /api/v3/openOrders, 合约 /fapi/v1/openOrders)

删除
  packaging/locus@.service                      # 不再注册 OS 服务
```

## 决策(已拍板)

| # | 决策 | 结论 |
|:--|:--|:--|
| D1 | daemon↔CLI 通道 | TCP 127.0.0.1 + OS 随机端口 + token(uuid, 写入 `run/daemon.json`, 权限 0600; 防同机其他进程/用户驱动下单) |
| D2 | 优雅停机通道 | stdin 管道指令(`stop\n`); EOF 视为 daemon 已消失 → 子进程自行清理退出(跨平台一致, 同时补上 Windows 无 SIGTERM 的短板) |
| D3 | 清理范围 | 策略 `on_stop`(可缺省) + 引擎撤单兜底 + 残留检查与手工处理提示; 不做 `--close-all`。<br>**范围修正(2026-09-12 收敛)**: 本期只落地 `on_stop` + 残留提示; **引擎撤单兜底延后**至实盘运行器 —— Dry Run 订单是虚拟撮合, 引擎撤单会撤到与本策略无关的真实挂单。`Exchange::get_open_orders` 作为兜底数据源已就绪。 |
| D4 | 数据目录默认值 | 平台标准目录(Win `%APPDATA%\locus` / mac `~/Library/Application Support/locus` / Linux `~/.local/share/locus`); 当前目录已有 `locus.db` 或 `strategies/` 时沿用现状 |

### 实现决策

| # | 议题 | 结论与依据 |
|:--|:--|:--|
| P1 | spawn 用哪个 API | `std::process::Command`, **不用 `tokio::process`** —— 实测: tokio 侧 spawn 后启动器退出会带走子进程; std 侧父进程退出子进程存活 |
| P2 | daemon 自后台化 | `std` spawn 自身 + `process_group(0)`(unix) / `creation_flags(DETACHED_PROCESS\|CREATE_NO_WINDOW)`(windows) —— 已实测 `forbid(unsafe_code)` 下编译可用 |
| P3 | 存活与等待 | daemon 侧 `Child::try_wait()`; `stop` 等待清理上限 30s(清理会调用交易所), 超时如实报错而非静默强杀 |
| P4 | 清理职责边界 | 清理由**策略进程**执行(它持有 exchange/ctx/密钥), daemon 只做进程管理(发指令/等退出) |
| P5 | 日志 | daemon spawn 时 stdout/stderr 追加重定向到 `logs/<name>.log`; 启动时若 >10MB 轮转为 `.log.1`(写入者是自己, 重命名安全) |
| P6 | 状态判定 | 无心跳: 运行中 = 子进程未退出; 已中止 = 记录 `last_exit` 与时间 |
| P7 | 命令语义 | `run` = 前台调试(进程内读 stdin, 支持手工 Ctrl-D/管道 EOF 退出); `start/stop` = 经 daemon, 是唯一被管理路径 |

## 改动清单(按依赖顺序)

1. **前置修复(无它则新功能是空壳)**
   - `Exchange::get_open_orders(pair)` + 现货/合约实现(demo 端点已可测)
   - run 循环: `drain_fills` 后 `insert_fill(strategy_id = 策略名)`
   - run 循环: 停机时调用 `strategy.on_stop(&mut ctx)`
   - `let _ = ctx.place_order(...)` 改为记录错误并计入状态
2. **supervisor 模块**: 台账(JSON 读写) → 子进程管理 → 协议 → server/client
3. **daemon 命令**: `start`(自后台化 + 写 daemon.json) / `status` / `stop`(优雅停全部策略后退出)
4. **CLI 命令面**: `list/status/info/fills` + `logs` 实装 + `ctrl.rs` 重写为客户端
5. **停机清理三段式**(引擎侧), 含"未实现清理"的提示文案与残留明细
   > **范围修正(2026-09-12 收敛)**: 本期实现 `on_stop` 调用 + 三种结局的如实提示文案; 引擎撤单兜底与残留明细延后到实盘运行器(依据见 D3 注)。
6. **文档同步**: `specs/architecture.md` §三 / §六、`specs/roadmap.md`、`specs/lua-api.md`(on_stop 时序与超时说明)
7. **删除** `packaging/locus@.service`

## 验证

- `cargo test --workspace` 全绿, 新增单元用例(协议/台账/状态判定/平台分支形状)不回归 204 基线
- `cargo check --target x86_64-pc-windows-msvc` 通过(Windows 分支类型检查)
- testnet 真实调用(`#[ignore]`, 需 env key, 见 `specs/testnet.md`):
  - demo 现货: 起策略 → 成交落库 → `fills` 可查 → `stop` → 撤单兜底后挂单为空
  - demo 合约: 同上 + 持仓残留时给出"需手工处理"提示
  - `kill -9` daemon → 策略进程经管道 EOF 自行清理退出(A4)
- 真实命令冒烟: `locus daemon start` → `locus list` → `locus start shannon_grid` → `locus info` → `locus logs -f` → `locus stop` → `locus daemon stop`(Linux 实测; Windows 分支仅编译级验证)
