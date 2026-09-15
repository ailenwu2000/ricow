# 功能规格: 跨平台策略进程管理(008-platform-process-model)

**功能目录**: `specs/changes/008-platform-process-model`

**创建日期**: 2026-09-12

**状态**: 方案已定稿(2026-09-12 用户拍板 D1-D4), 待实施

**输入**: 用户 2026-09-12 指令(跨平台 / 简单易用 / 管理器退出即中止子进程 / 清理与手工提示);
实测证据: `specs/research/process-model-probe-2026-09.md`, 可复跑探针: `specs/changes/008-platform-process-model/probes/`

## 一、问题(现状逐份核对)

| # | 现状 | 依据 |
|:--|:--|:--|
| 1 | 进程管理只走 systemd user 转发 | `crates/locus_cli/src/commands/ctrl.rs` 全文(systemctl 判定/转发) + `packaging/locus@.service`(含 `Restart=on-failure`) → macOS/Windows 不可用, 且自动重启与"出错即中止"相反 |
| 2 | 无实例台账 | 策略进程退出后无退出码/退出时间记录; `locus status` 只能反映 systemd 实例 |
| 3 | `locus logs` 是空壳 | `crates/locus_cli/src/commands/logs.rs:14` 仅打印提示 |
| 4 | 停机清理不存在 | `on_stop` 已在 trait(`crates/locus_strategy/src/strategy.rs:21`)与 Lua 绑定(`lua.rs:521`), `specs/lua-api.md:18` 已承诺"策略停止时调用"; 但 run 循环(`crates/locus_engine/src/command.rs:35-53`)从不调用 |
| 5 | 成交不落库 | `fills` 表有 `strategy_id` 列(`db.rs:67`), 但 `insert_fill`(`db.rs:298`)在生产路径无调用点(仅定义与单测) → "查看成交记录"无数据源 |
| 6 | 无未成交挂单查询能力 | `Exchange` trait(`crates/locus_core/src/exchange.rs`)只有 place_order/cancel_order/get_position/get_balance → 清理兜底与"残留提示"缺数据来源 |
| 7 | 下单错误被吞 | `command.rs:46` `let _ = ctx.place_order(req)` → 下单/清理失败静默 |

## 二、范围

### 做

1. **常驻进程管理器(daemon, pm2 模型)**: 持有策略子进程; daemon 退出时子进程全部中止
2. **命令面**: `daemon start|stop|status` / `list|status` / `start <name>` / `stop <name>` / `info <name>` / `fills [name]` / `logs <name> [-f]`
3. **实例台账与日志**: `run/daemon.json`(pid/端口/token) + `run/<name>.json`(实例冗余台账) + `logs/<name>.log`
4. **跨平台统一**: Linux / macOS / Windows 同一套代码与语义, **零新增依赖**(std 进程 API + tokio + 既有 serde_json/uuid)
5. **优雅停机通道**: daemon→策略进程 stdin 管道指令; 管道 EOF(长度 0) = daemon 已不在 → 策略进程自行清理并退出
6. **停机清理三段式**: 策略 `on_stop`(可缺省) → 引擎撤单兜底 → 残留检查与如实提示
   > **范围修正(2026-09-12 收敛)**: 本期落地 `on_stop` + 残留提示两段; **引擎撤单兜底延后**到实盘运行器变更 —— Dry Run 的订单是虚拟撮合, 从未到交易所, 引擎若去交易所撤单会撤到与本策略无关的真实挂单(危险且虚假)。
7. **前置修复**: `insert_fill` 接线 / `on_stop` 调用 / 下单错误不吞 / `Exchange::get_open_orders`

### 不做

- OS 服务注册(systemd / launchd / Windows SCM)与开机自启、自动重启
- 进程表扫描发现"外部启动"的实例(daemon 是唯一启动者; `locus run` 前台保留为调试入口, 不被管理)
- 心跳机制(daemon 持有 Child 句柄, 存活 = 子进程未退出)
- 进程管理库依赖: 生态无成熟跨平台管理器(Rust 侧只有 process-wrap/sysinfo 一类"进程创建/查询"库), 自实现更省
- 每策略独立数据库(`fills` 按 `strategy_id` 过滤即可)
- `locus stop --close-all`(强制平仓) —— 保留为后续选项, 本期只提示手工处理

## 三、用户场景与验收标准

### 场景

- **US1 一眼看到在跑什么**: `locus list` 输出 全部已部署策略 × 状态(运行中 / 已中止 exit=N / 未运行) × 模式(Dry Run / 实盘) × 交易对 × PID × 运行时长
- **US2 启停**: `locus start <name>`(已在运行则报错退出, 不重复拉起); `locus stop <name>`(等待清理完成, 超时如实报错)
- **US3 停机清理**: 三段式执行; 策略未实现 `on_stop` 时明确提示"该策略未实现清理, 请手工处理", 并列出当前持仓与未成交挂单明细
  > **范围修正(2026-09-12 收敛)**: 持仓/挂单明细依赖实盘账户接口, 本期未实现(见 tasks 收尾待办 2/3), 本期以"如实提示 + 不臆测"为准。
- **US4 查成交与日志**: `locus fills <name> [--limit N]` 按 `strategy_id` 过滤; `locus info <name>` 给出运行信息(模式/交易对/启动时刻/运行时长/累计成交笔数/手续费/最近成交时间/当前持仓); `locus logs <name> -f` 尾随
  > **范围修正(2026-09-12 收敛)**: `info` 本期给出成交笔数/手续费/最近成交 + 日志路径; **不含当前持仓**(Dry Run 无真实持仓, 展示臆测数字是虚假信息)。
- **US5 daemon 生命周期**: `locus daemon start` 一次自后台化; `locus daemon stop` 优雅停全部策略后退出; daemon 崩溃或被强杀 → 策略进程读到管道 EOF 自行清理退出

### 验收标准

| # | 标准 | 验证方式 |
|:--|:--|:--|
| A1 | 三平台编译通过 | Linux 本机构建 ✅ + `cargo check --target x86_64-pc-windows-msvc`(**Windows 验证当前不可复现**, 见 §四); macOS 走 `cfg(unix)` 分支, **无实机, 须在文档与依赖说明中标注未实机验证** |
| A2 | 单元测试覆盖协议解析 / 台账读写 / 状态判定, 不触碰真实资金 | `cargo test --workspace` |
| A3 | 停机清理经 testnet 真实调用验证 | `#[ignore]` 用例: demo 现货 + 合约 起策略 → 成交落库 → `stop` → 撤单兜底生效(挂单查询为空) → 无 `on_stop` 的策略给出残留提示<br>**范围修正(2026-09-12)**: 本期已验证「挂单查询闭环」(现货/合约 openOrders 真实调用) + 「拒单如实上报」; 撤单兜底与残留明细延后到实盘运行器(见 D3 注)。 |
| A4 | daemon 崩溃自愈 | `kill -9` daemon 后, 策略进程在清理超时上限内自行退出, 日志留痕 |
| A5 | 不回归既有基线 | `cargo test --workspace` 全绿(实施前 204 passed / 0 failed / 6 ignored; 实施后 **216 passed / 0 failed / 9 ignored**) |

## 四、依赖与风险

- **A1 的 Windows 编译级验证当前不可复现 (2026-09-12 收敛实测)**: 本机 WSL 未装 MSVC 工具链(`lib.exe` 缺失),
  `cargo check --target x86_64-pc-windows-msvc --workspace` 在 cc-rs 处失败; 换 `x86_64-pc-windows-gnu`(rust-std 已装)则报
  `can't find crate for core` —— 工具链目录名为 `1.83-x86_64-unknown-linux-gnu` 而实际 `rustc 1.96.1`, sysroot 不匹配, 属环境问题。
  故 A1 的 Windows 结论降级为**当前未验证**, `cfg(windows)` 分支仅经代码评审; 恢复方式: 在装有 MSVC 工具链的 Windows 宿主
  (或本机 `apt install gcc-mingw-w64-x86-64` 后用 `--target x86_64-pc-windows-gnu`)重跑 `cargo check --workspace`。
- **macOS 无实机**: 仅能保证 `cfg(unix)` 分支与 Linux 同源, 不在本变更内宣称实机验证
- **MSRV 现状**: workspace 声明 `rust-version = 1.83`, 但实测 `cargo 1.83` 已无法解析当前依赖(indexmap 2.14 要求 edition2024), 实际构建用 stable; 本变更零新增依赖, 不受影响 —— MSRV 声明修正另行处理, 本档案不引入兼容降级逻辑
- **清理可能失败**: 交易所拒单/网络异常时, 清理结果是"未完成 + 明确提示", 不做自动重试
