# 008 任务分解

> 顺序即依赖顺序; 每项完成须有可验证判据。交易与清理相关验收一律币安 demo 真实调用(宪法原则三)。
> 基线: 实施前 `cargo test --workspace` = 204 passed / 0 failed / 6 ignored; 实施后 **216 passed / 0 failed / 9 ignored**(含 A3 新增的真实拒单用例)。
> 状态: 2026-09-12 实施完成(阶段 1-5), 阶段 6 验收全项完成 —— A3 的 testnet 真实调用 3 例(现货闭环 / 合约闭环 / 拒单上报)已跑绿。

## 阶段 1 — 前置修复(无它则新功能是空壳)

- [x] **T001 Exchange 增挂单查询**
  内容: `Exchange` trait 增 `get_open_orders(pair) -> Vec<OrderInfo>`; `locus_binance` 现货(`GET /api/v3/openOrders`)与合约(`GET /fapi/v1/openOrders`)实现, 走既有签名客户端与 demo 域名配置。
  判据: 单元测试(解析) + `#[ignore]` demo 真实调用返回结构正确; 不 mock。
  完成: `locus_core::OrderInfo` + `Exchange::get_open_orders` + `BinanceClient::get_open_orders` + `FuturesClient::get_open_orders`(复用 `spot::parse_open_orders`); 单测 `test_parse_open_orders` 通过。

- [x] **T002 成交落库接线**
  内容: run 循环 `drain_fills` 后 `db.insert_fill(strategy_id, fill)`, `strategy_id` 取策略名(`config.name`); 失败按 T004 的错误策略处理。
  判据: demo 现货起策略真实成交后, `SELECT * FROM fills WHERE strategy_id=...` 有记录且字段与交易所一致。
  完成: `run_dry_run` 增 `db: Option<&Database>`; 真实冒烟(ETHUSDT)37 笔成交全部落库, `strategy_id` = 策略名, 含停机清理阶段产生的成交。
  附带修复: run 的 Dry Run 初始虚拟资金原为固定 `USDC`, 与 USDT 报价对不匹配(策略判定"无可用资金")→ 改为按交易对报价资产配平(`quote_asset_of`)。

- [x] **T003 on_stop 调用接线**
  内容: run 循环在停机时调用 `strategy.on_stop(&mut ctx)`; `on_stop` 内允许 `ctx.place_order`(撤单/平仓), 其产生的成交同样落库。
  判据: 内置脚本未定义 `on_stop` 时不报错(空实现); 自定义脚本定义后 demo 真实调用可见其下单生效并落库。
  完成: 停机序 = `strategy.on_stop()` → 清理成交落库 + `on_fill`; 所有退出路径(停机指令/管道 EOF/Ctrl-C/行情流中断)均调用。
  说明: **停机回调不做引擎侧超时** —— Lua 侧本有指令预算(1M/tick)约束; 超时保护由 daemon 侧等待上限(30s)承担并如实报错, 不重复实现。

- [x] **T004 下单错误不再吞掉**
  内容: `command.rs` 的 `let _ = ctx.place_order(req)` 改为: 失败记录 `tracing::error` + 计入运行结果(错误计数/最近错误)。
  判据: 构造必然失败的订单, stop/run 输出与状态如实反映失败, 无静默。
  完成: `RunOutcome{orders_submitted, order_errors, persist_errors, last_error}`; 行情流中断不再静默 `Ok(())`(改为非零退出 + 日志), CLI 如实打印统计。
  真实失败用例(2026-09-12 A3): `demo_futures_rejected_order_surfaces_error` —— 故意提交违反对齐的合约限价单,
  交易所返回 `BN 400 ... Precision is over the maximum defined for this asset.` 被**如实抛出**(Err 携带原始原因、
  非超时、非静默), 且交易所无残单。引擎层 `order_errors` 计数随实盘运行器落地(本期 Dry Run 订单是虚拟撮合, 不经交易所)。

## 阶段 2 — supervisor 模块(daemon 主体)

- [x] **T005 实例台账**: `run/daemon.json`(pid/端口/token, 0600) + `run/<name>.json`(pid/启动时间/模式/交易对/上次退出码与原因); 坏文件忽略并告警。单测 2 例(权限 0600 / 坏文件与 daemon.json 不入台账)。
- [x] **T006 子进程管理**: `std::process` spawn `locus run <name>`; stdin 管道; stdout/stderr 追加重定向 `logs/<name>.log`(启动时 >10MB 轮转 `.log.1`); `stop` 指令; `try_wait`; 等待上限 30s。单测 4 例。
- [x] **T007 行协议 + token 校验**: 单行 JSON(`cmd` tag + `token`), 指令 ping/list/start/stop/info/shutdown; 错 token 拒绝。单测 5 例(往返/扁平形状/坏 JSON/未知指令/错 token 拒绝由集成测试覆盖)。
  简化说明: 计划中的 `logs_meta` 指令取消 —— `locus logs` 直接读 `logs/<name>.log`, daemon 不在也能看日志, 无需协议往返。
- [x] **T008 TCP server + 指令分发**: 仅绑 127.0.0.1 + 随机端口; 停机先优雅停全部策略再退出; 后台监控任务(1s)发现自行退出的子进程并落台账。集成测试: 真实 TCP 回环(非 mock)覆盖 ping/list/错 token/幂等 stop/shutdown 后清理 `daemon.json`。
- [x] **T009 CLI 客户端**: 读 `run/daemon.json` 连接并带 token; daemon 不在时明确报错(不自动拉起、不静默失败)。

## 阶段 3 — CLI 命令面

- [x] **T010 `daemon start|stop|status`**: `start` 自后台化(`process_group(0)` / `creation_flags(DETACHED_PROCESS|CREATE_NO_WINDOW)`), 陈旧 `daemon.json` 自动清理; `stop` 发 shutdown 并等待(上限 90s); `status` 显示 pid/端口/运行中实例。
- [x] **T011 `list` / `status`**: 已部署 TOML × 状态 × 模式 × 交易对 × PID × 运行时长 × 备注(退出原因 / 无 TOML / 配置声明实盘但仅 Dry Run); 汇总行。
- [x] **T012 `start` / `stop`**: `start` 经 daemon(已在运行/`enabled=false` 拒绝); `stop` 等待清理并按退出码如实报告; 重复 stop 幂等。
- [x] **T013 `info` / `fills` / `logs`**: `info` = 状态/PID/运行时长/模式/交易对/配置/成交统计/日志路径; `fills [name] [--limit N]` 按 `strategy_id` 倒序; `logs <name> [-f] [--lines N]` 轮询尾随(200ms, 无额外依赖)。
  范围修正: **当前持仓不纳入本期** —— 持仓查询需密钥账户接口, 且 Dry Run 无真实持仓; `info` 改为如实展示"成交/手续费/最近成交 + 日志路径", 不展示臆测的持仓数字。余下"持仓/挂单快照"随实盘运行器一并落地。
- [x] **T014 `ctrl.rs` 重写**: 删除 systemctl 判定/转发与相关单测; `start/stop/restart` 改走 daemon 客户端; 非 Linux 不再报"systemd 不可用"。

## 阶段 4 — 停机清理

- [x] **T015 清理流程(按 Dry Run 现实收敛)**
  内容: 停机序 = 策略 `on_stop`(定义了才调, 引擎判定并如实提示) → 清理成交落库 → 结果输出。
  判据: demo 现货 stop 后进程退出、日志留痕、`stop` 输出如实说明"是否实现清理"。
  范围修正: **引擎撤单兜底不进本期** —— Dry Run 的订单是虚拟撮合, 从未到交易所, 引擎若去交易所撤单会撤到**与本策略无关的真实挂单**(危险且虚假)。故改为: (a) T001 的 `get_open_orders` 作为实盘兜底的数据源已就绪; (b) 兜底逻辑随实盘运行器落地; (c) 本期以"如实提示 + 不臆测清理结果"为准。
- [x] **T016 停机提示文案**: 三种结局如实表述 —— 已实现清理(结果以日志为准) / 未实现清理(需手工处理) / 无法判定(装载失败, 请手工核对); 文案经逐句对照代码行为复核, 不含"已平仓"等未发生的事实。

## 阶段 5 — 数据目录与文档

- [x] **T017 平台标准数据目录**(D4): 显式 `LOCUS_ROOT` > 现状回退(目录已有 `locus.db`/`strategies/`) > 平台标准目录(Win `%APPDATA%\locus` / mac `~/Library/Application Support/locus` / Linux `$XDG_DATA_HOME|~/.local/share/locus`); 决策抽为纯函数 `resolve_root` 并单测 4 条规则。
- [x] **T018 文档同步**: `architecture.md` §三(进程模型整节重写)/§五(命令集)/§六(数据布局: `run/` `logs/` + busy_timeout 约束)/§九(基线 216)/§十一(残留清理 7 项已清, 3 项仍存); `lua-api.md`(on_stop 清理语义); `roadmap.md`(008 状态 + 基线); `README.md`/`README_zh.md`(packaging → supervisor)。
- [x] **T019 删除 systemd 单元**: `packaging/locus@.service` 已删(目录一并移除); 全仓引用清理(README×2 + architecture §三)。

## 阶段 6 — 验收

- [x] **T020 单元与基线**: `cargo test --workspace` = 216 passed / 0 failed / 9 ignored; 新增用例均为纯逻辑或真实 TCP 回环, 无 mock 替身。
- [x] **T021 真实调用冒烟(等价于 A3 的 Dry Run 部分)**: 隔离 `LOCUS_ROOT` 下跑通 daemon/list/start/info/fills/logs/stop 全链路, 真实行情 + 37 笔真实成交落库; 停机退出码 0。
  实测(A3 testnet 部分, 2026-09-12):
  - **现货闭环 ✅ 真实通过**(`crates/locus_binance/tests/demo_open_orders_live.rs`): demo 现货 BTCUSDT 挂远离市价限价单 → `Exchange::get_open_orders` 真实调用返回该单且字段全部核对通过(side/pair/price/size/status/filled=0/orderId 非空) → 撤单 → 再查已消失。多次重跑均通过。
  - **合约闭环 ✅ 真实通过**(同文件 `demo_futures_open_orders_roundtrip`): one-way + 1x, fapi 挂单 → `FuturesClient::get_open_orders` 返回该单且 side/price/size 全部核对通过 → 撤单 → 再查已消失。
    关键: 价格必须按 `PRICE_FILTER.tickSize` 向下对齐(测试原用 `round_dp(2)`, 合约 tickSize=0.1 → `Price not increased by tick size` 被拒); 已抽 `price_on_tick()` 并两市场共用。
  - **拒单如实上报 ✅ 真实通过**(`demo_futures_rejected_order_surfaces_error`): 故意提交违反对齐的合约单 → 交易所 `BN 400 ... Precision is over the maximum defined for this asset.` 被如实抛出(Err 带原始原因、非超时), 无残单。
  - 环境时钟处理(2026-09-12 复盘): `wsl --shutdown` **未能**修复 WSL2 时基(单调钟仍快 ~5.25%, 壁钟靠 NTP 每 ~32s 拽回, 锯齿峰值 >+1000ms 被币安硬拒)。
    可用做法(每轮测试前必做, 且**现货/合约分开跑、各自对齐自己的 demo 时间**):
    ```bash
    # 合约组: 对齐合约 demo 自身时间(滞后 1s 留安全余量)
    python3 -c "import json,subprocess,time,urllib.request as u; t=json.load(u.urlopen('https://demo-fapi.binance.com/fapi/v1/time'))['serverTime']/1000-1.0; subprocess.run(['sudo','date','-s',time.strftime('%Y-%m-%d %H:%M:%S',time.localtime(t))+'.000'])"
    # 现货组: 现货交易节点时钟比 /api/v3/time 快约 2s, 故滞后 3s
    python3 -c "import json,subprocess,time,urllib.request as u; t=json.load(u.urlopen('https://demo-api.binance.com/api/v3/time'))['serverTime']/1000-3.0; subprocess.run(['sudo','date','-s',time.strftime('%Y-%m-%d %H:%M:%S',time.localtime(t))+'.000'])"
    ```
    滞后方向有 5s recvWindow 容差, 是安全侧; 超前 >1000ms 才是硬拒。全部 3 个用例均在同轮内一次通过。
- [x] **T022 崩溃自愈(A4)**: `kill -9` daemon 后策略进程 4s 内自愈退出(读 stdin 管道 EOF), 无孤儿进程; 陈旧 `daemon.json` 在下次 `daemon start` 时自动清理。
- [x] **T023 真实命令冒烟**: Linux 完整冒烟通过; Windows 仅编译级验证; macOS 未实机验证(如实标注在 spec §四)。

## 收尾待办(不阻塞 008 归档)

1. A3 的 testnet 真实撤单/持仓核对用例(需 demo key)
2. 实盘运行器 + 实盘撤单兜底 + `stop --close-all`(另立变更)
3. `info` 的持仓/挂单快照(依赖实盘账户接口)

## Phase 7: Convergence

> 2026-09-12 收敛评估(/speckit-converge): 对照 spec / plan / tasks 与当前代码, 上述 3 项为可执行缺口。

- [x] T024 修订 spec.md / plan.md 文本以对齐已收敛范围: 停机清理的「引擎撤单兜底」(A3 / D3 / plan 改动清单 5) 与 US3 残留明细、US4 `info` 当前持仓在本期**有意未实现**(Dry Run 订单为虚拟撮合, 引擎去交易所撤单会撤到与本策略无关的真实挂单) —— 就地表注范围修正与依据, 实现随实盘运行器变更落地 per A3+D3+plan:改动清单5 (partial)
  完成(2026-09-12): spec §二6 / §三 US3 / US4 / A3 / A5 与 plan D3 / 改动清单 5 均已就地表注范围修正与依据; A5 基线同步为 216/0/9。
- [x] T025 恢复 A1 的 Windows 编译级验证: `rustup target add x86_64-pc-windows-msvc` + `cargo check --target x86_64-pc-windows-msvc --workspace`(需 MSVC 工具链, 当前报 cc-rs 找不到 `lib.exe`); 若本机确实不可得, 则把 spec §四 从「Windows 编译级验证」如实降级为「未验证」, 不留失效证据 per A1 (partial)
  完成(2026-09-12, 走降级分支): 本机 MSVC 工具链不可得(`lib.exe` 缺失); GNU target 亦因工具链 sysroot 与 rustc 版本不一致报 `can't find crate for core`。已按备选在 spec §四 如实降级并写明恢复方式, 不留失效证据。
- [x] T026 更新 `.specify/feature.json` 指向当前活跃变更: 现指向已下线的 `007-bs-momentum-v2`, 致 `check-prerequisites.sh` 报 `plan.md not found`, 后续 speckit 流程会定位到错误档案 per plan:项目结构 (partial)
  完成(2026-09-12): 已改为 `specs/changes/008-platform-process-model`。
