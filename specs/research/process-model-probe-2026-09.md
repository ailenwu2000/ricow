# 进程模型实测定稿 (2026-09-12)

> 定位: `specs/changes/008-platform-process-model` 方案的证据档案(实测事实, 非方案讨论)。
> 探针脚本: `specs/changes/008-platform-process-model/probes/`(可复跑)。
> 环境: WSL2 Linux(WSL 内核) + rustc/cargo stable 1.96.1 与 1.83 双工具链; Windows 侧仅 `cargo check --target x86_64-pc-windows-msvc`(本机已装该 target)。

## 一、平台 API 在 `unsafe_code = "forbid"` 下是否可用

| 能力 | API | 平台 | 验证 | 结果 |
|:--|:--|:--|:--|:--|
| 子进程脱离终端进程组 | `std::os::unix::process::CommandExt::process_group(0)` | Linux/macOS | 编译(forbid unsafe) + 运行 | ✅ safe |
| Windows 后台/无窗口启动 | `std::os::windows::process::CommandExt::creation_flags(DETACHED_PROCESS\|CREATE_NO_WINDOW\|CREATE_NEW_PROCESS_GROUP)` | Windows | `cargo check --target x86_64-pc-windows-msvc` | ✅ safe |
| Unix 新会话(setsid) | `process_wrap::std::ProcessSession` | POSIX | 运行: 子进程 `sid == pid` | ✅ 真 setsid(决策未采用该库, 见 §六) |

结论: 平台分支无需任何 `unsafe`, 与 workspace lint 兼容。

## 二、后台进程存活 / 生命周期(决定性实测)

| # | spawn 方式 | 启动器行为 | 子进程结果 |
|:--|:--|:--|:--|
| 1 | `std::process` + `process_group(0)` | 父进程立即退出 | **存活** ✅(被 init 收养, `ps` 可见) |
| 2 | `process-wrap::std` + `ProcessSession` | 父进程立即退出 | **存活** ✅(且 `sid == pid`) |
| 3 | `tokio::process`(含 `process-wrap::tokio`) | runtime 立即关闭 | **被带走 ❌**(两次复现) |
| 4 | `#[tokio::main]` + `std::process` + `process_group(0)` | 父进程立即退出 | **存活** ✅ |

- 结论: 进程管理走 **std 侧 spawn**; 即便 CLI 本体是 `#[tokio::main]`, 用 `std::process` 亦安全(#4)。
- #3 与 tokio 文档("默认 drop 不杀子进程、runtime 尽力回收")不符; 机制未逐行定位, 但现象稳定复现, 按实测结论使用。
- 复现脚本: `probes/proc-probe-main.rs`(#1)、`probes/pmprobe-stdcheck.rs`(#2)、`probes/pmprobe-tokiocheck.rs`/`pmprobe-main.rs`(#3)、`probes/pmprobe-tokiostd.rs`(#4)。

## 三、停机通道(D2 核心机制, 实测通过)

`probes/pipecheck.rs`(纯 std): daemon spawn 子进程时把 stdin 接成管道, 子进程在 run 循环里读该管道。

```
(a) 正常停机
  daemon: spawned child pid=227606
  child  pid=227606 开始运行, 监听 stdin
  child: 收到 stop 指令 → 执行清理 (撤单/平仓) → 退出
  daemon: child 已退出 status=ExitStatus(unix_wait_status(0))

(b) daemon 被 kill -9(不写指令, 管道写端随进程消失)
  子进程日志: child: stdin EOF (daemon 已不在) → 自行执行清理 → 退出
  子进程是否仍存活: False
```

- 一根管道同时承担"优雅停机指令"与"父进程存活探测", 跨平台语义一致(Windows 无 SIGTERM, 此机制不依赖信号)。
- 依据: 操作系统**不会**在父进程死亡时带走子进程(§二 #1 已证: 父退出子进程仍存活), 故"管理器退出 → 子进程中止"必须由机制保证, 不能依赖 OS。

## 四、SQLite 多进程并发写(3 进程 × 200 独立事务)

| 数据目录 | busy_timeout | 期望 | 实际写入 | SQLITE_BUSY 失败 | 墙钟 |
|:--|:--|--:|--:|--:|--:|
| /tmp (本机 ext4) | 0 ms | 600 | 103 | 497 | 0.16s |
| /tmp (本机 ext4) | 5000 ms | 600 | **600** | 0 | 0.91s |
| /mnt/d (WSL drvfs) | 0 ms | 600 | 73 | 527 | 0.25s |
| /mnt/d (WSL drvfs) | 5000 ms | 600 | **600** | 0 | 1.82s |

- WAL + `busy_timeout` 下多进程并发写全部成功, **不需要更换数据库**; 失败仅出现在 busy_timeout=0。
- `sqlx` 0.8.6 默认 `busy_timeout = 5s`(源码 `sqlx-sqlite-0.8.6/src/options/mod.rs:201`), 现有 `Database::open`(crates/ricow_strategy/src/db.rs:26-30) 已显式设 WAL → 当前配置即满足; 该依赖须写入文档, 防止后人改成 0 或用"加大连接池"处理。
- SQLite 官方 WAL 硬限制: 依赖同机共享内存, "the write-ahead log implementation will not work on a network filesystem"(sqlite.org/wal.html) → 数据目录禁放网络盘/云同步盘; WSL 挂 /mnt/d 可跑但吞吐约慢 2×(实测)。
- 复现脚本: `probes/sqlite_conc_probe.py`。

## 五、进程识别(argv 扫描, 若需发现外部实例时可用)

- `sysinfo` 可读到 `name / exe / cmd(完整 argv) / start_time(秒级 unix 时间)`, 足以识别 `ricow run <name>` 并防 PID 复用误判。
- ⚠️ 实测踩坑: `System::new()` + `refresh_processes(All, true)` **不填充 cmd/exe**(候选命中 0); 必须 `System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()))`。
- ⚠️ Windows 读其他进程 cmd 受权限限制(提权进程不可读)。
- 008 最终方案未采用进程扫描(daemon 是唯一启动者, 持 Child 句柄即知存活), 本条留档备后用。脚本: `probes/proc-probe-scan.rs`、`probes/proc-probe-dbg.rs`。

## 六、生态调研结论(为何零新增依赖)

crates.io 搜索 + GitHub 数据: Rust 生态**没有**成熟的跨平台"进程管理器产品"。

| 候选 | 数据 | 结论 |
|:--|:--|:--|
| process-wrap 10.0.0 | watchexec 团队, 13.4M 下载, MSRV 1.87 | 只解决"把命令放进进程组/会话/job"; 且其 tokio frontend 命中 §二 #3 陷阱 |
| command-group 5.0.1 | 同上作者, 7.3M 下载 | 已官方标记 Deprecated → 用 process-wrap |
| sysinfo 0.39.6 | 2.7k stars, 199M 下载, MIT | 查询/终止进程; 本项目方案不需要(daemon 持句柄) |
| win32job 2.0.3 | 1.3M 下载 | Windows Job Object: 语义是"父死子死", 与需求相反 |
| supervisor-rs 0.8.5 | 90 stars, 2024-03 后停更 | 不采用 |
| gaffa / ultraman | 1 / 33 stars | Procfile 同类, 生态过小 |
| daemonize 0.5 / fork 0.10 | 17M / 6.4M 下载 | Unix only |
| service-manager-rs 0.11 | 245 stars | 注册 OS 服务: 提权 + 与"不要自动重启"矛盾 |
| pm2(非 Rust) | 43.3k stars | 跨平台但需 Node 运行时, 破坏单二进制分发 |
| supervisord(非 Rust) | 9.1k stars | Unix only |

→ 自实现常驻 daemon(约 400-500 行, 零新增依赖) 是当前生态下的合理选择。

## 七、MSRV 现状(与 008 无关, 但属实测发现, 供后续立项)

- workspace 声明 `rust-version = "1.83"` + `rust-toolchain.toml` 固定 1.83, 但实测 `rustup run 1.83 cargo build` **失败**:
  `indexmap 2.14.2` 要求 `edition2024` → "feature `edition2024` is required"(cargo 1.83 未稳定该特性)。
- 即实际构建用的是 stable(本地 1.96.1), 1.83 声明已名存实亡。008 零新增依赖, 不受影响; MSRV 声明是否修正另行立项(不在 008 内加降级逻辑)。

## 八、探针使用注意(踩坑留痕, 便于复跑)

- `pkill -f '<pattern>'` 会匹配到执行它的 shell 自身, 导致自杀中断输出 → 用 `pkill -x <进程名>`。
- Rust lint 对 `async fn`/edition 的 E0670 在本机为**误报**(以 `cargo build` 为准)。
- 探针输出经管道时是块缓冲, 进程被信号杀掉会丢失未 flush 的输出 → 用 `stdbuf -oL` 或写入文件。
