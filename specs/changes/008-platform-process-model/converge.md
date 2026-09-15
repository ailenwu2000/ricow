# 008 收敛核对 (converge)

日期: 2026-09-12

| 验收标准 | 实测 | 结论 |
|:--|:--|:--|
| A1 三平台编译 | Linux `cargo build --workspace` 通过; Windows 编译级验证**当前不可复现**(MSVC 缺 `lib.exe`; GNU target 报 `can't find crate for core` —— 工具链目录 1.83 与实际 rustc 1.96.1 sysroot 不一致), 已在 spec §四如实降级; macOS 无实机 | ⚠️ 部分 |
| A2 单元测试 | `cargo test --workspace` = 216 passed / 0 failed / 9 ignored | ✅ |
| A3 testnet 真实调用 | 现货 openOrders 闭环 ✅ / 合约 openOrders 闭环 ✅ / 拒单如实上报 ✅(3 例均一次跑绿, 见 `crates/locus_binance/tests/demo_open_orders_live.rs`); **撤单兜底与持仓/挂单明细本期有意未实现**(Dry Run 订单是虚拟撮合, 引擎撤单会撤到与本策略无关的真实挂单), 已在 spec A3/D3 与 plan 改动清单 5 就地标注 | ⚠️ 部分(有意) |
| A4 daemon 崩溃自愈 | `kill -9` daemon 后子进程 4s 内经管道 EOF 自愈退出, 无孤儿进程 | ✅ |
| A5 不回归基线 | 204/0/6 → 216/0/9(新增 supervisor/CLI 用例 + 真实拒单用例) | ✅ |
| 宪法原则一~五 | 本机 TCP 127.0.0.1+token 无遥测 / 无新增 Rust 策略本体 / 真实调用禁 mock / 产物中文 / 零新增外部 crate(`uuid` 为既有 workspace 依赖) | ✅ |
| 计划决策 D1-D4、P1-P7 与改动清单 1-7 | 逐项核对: 命令面 12 个子命令齐备、`STOP_WAIT=30s`、>10MB 轮转 `.log.1`、std spawn 未用 `tokio::process`、无心跳、未扫进程表、`packaging/locus@.service` 已删、fills/on_stop/错误不吞三处接线在位 | ✅ |

## 收敛动作 (Phase 7)

评估发现 3 项可执行缺口, 已全部处置:

| 任务 | 处置 |
|:--|:--|
| T024 spec/plan 文本对齐范围修正 | ✅ spec §二6 / US3 / US4 / A3 / A5 + plan D3 / 改动清单 5 就地标注依据; A5 基线同步 216/0/9 |
| T025 恢复 Windows 编译级验证 | ✅ 走降级分支: 本机 MSVC 工具链不可得, spec §四 如实降级为「当前未验证」并写明恢复方式, 不留失效证据 |
| T026 更新 `.specify/feature.json` | ✅ 由 `007-bs-momentum-v2` 改为 `008-platform-process-model`; `check-prerequisites.sh` 已验证解析正确 |

## 结论

**本期范围已收敛**。收敛遗留(不阻塞归档、不属本期范围):

1. 引擎撤单兜底 + `stop --close-all` + `info` 持仓/挂单快照 —— 随**实盘运行器**变更落地(需账户接口; Dry Run 下做会撤到无关真实挂单)。
2. A1 的 Windows 编译级验证 —— 需在装有 MSVC 工具链的宿主上重跑 `cargo check --target x86_64-pc-windows-msvc --workspace`。

## 留档

- 时钟坑(本次踩到 8 次连续失败): WSL2 单调钟比服务器快 ~5.25%, 壁钟被 NTP 每 ~32s 拽回, 偏差锯齿峰值 >+1000ms 即被币安硬拒;
  `wsl --shutdown` 实测**无效**。可用做法 = 跑前 `sudo date -s` 压到滞后, 且现货/合约分开跑、各自对齐自己的 demo 时间(详见 `specs/testnet.md §注意事项`)。
- 合约下单硬约束: 价格须按 `PRICE_FILTER.tickSize` 对齐(测试原用 `round_dp(2)` 被拒), 数量须按 `LOT_SIZE.stepSize` 对齐。
  **产品侧当前无对齐逻辑**, 对齐责任在策略 Lua —— 实盘前应定口径。
