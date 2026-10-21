# 060 任务清单

- [x] T1 spec.md 落盘
- [x] T2 ricow_core: Exchange trait earn 三默认方法
- [x] T3 ricow_binance: earn.rs SAPI + client.rs pub(crate) + lib.rs + spot.rs 覆写
- [x] T4 ricow_strategy/context.rs: Context trait + LiveContext(TTL+吞错) + DryRunContext
- [x] T5 ricow_strategy/backtest.rs: virtual_earn + 四处权益聚合点加腿
- [x] T6 ricow_strategy/lua.rs: earn 快照 + _RICOW_EARN_OPS + drain_earn_ops
- [x] T7 strategies/spot/smart_position.lua + .toml
- [x] T8 注册: catalog.rs / builtin_tests.rs / .gitignore
- [x] T9 单元测试全绿 + 架构守卫 + clippy + fmt
- [x] T10 SOLUSDT 1 年真实回测(§〇 v1 全量汇报) — 附: 回测暴露报告 final_equity 丢理财腿 bug, 已修 + 回归锚点
- [ ] T11 demo 测试网 Dry Run — **受阻于本机网络**(*.binance.com DNS 污染, demo-api 不可达), 待网络恢复按 specs/testnet.md 复跑
- [x] T12 文档收敛: lua-api.md / roadmap / converge.md
