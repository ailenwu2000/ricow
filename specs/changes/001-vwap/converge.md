# 收敛报告: VWAP 执行模式示例

**日期**: 2026-09-01 | **变更**: specs/changes/001-vwap/

## 收敛结论: ✅ Converged

对照 spec.md / plan.md / tasks.md 逐项核验, 全部满足, 无遗留任务。

## 需求核验

| 需求 | 状态 | 证据 |
|:--|:--|:--|
| FR-001 --strategy vwap 直跑 | ✅ | BUILTIN_SCRIPTS 注册 + backtest/run inline_config; 真实回测冒烟通过 |
| FR-002 分片节奏 | ✅ | exec.slice_due; test_vwap_slices 断言首片立即/间隔/发完 |
| FR-003 VWAP 无前视计算 | ✅ | 只用已收盘 K 线(BacktestContext::klines = closed_klines); test_vwap_volume_weighted 断言 175 |
| FR-004 限价/回退市价 | ✅ | 首片无历史 → 市价; 有数据 → VWAP 限价; 成交量全 0 → 回退 close(test_vwap_zero_volume_fallback) |
| FR-005 klines volume 字段 | ✅ | lua.rs row.set("volume"); specs/lua-api.md 同步 |
| FR-006 参数校验/默认 | ✅ | lookback_bars 默认 20, num_slices≥1 守卫, ticks_per 守卫 |

## 成功标准核验

- SC-001: `cargo test --workspace` 全过, 154 测试(基线 151 + 3 新增)✅
- SC-002: `locus backtest --strategy vwap --pair BNBUSDT --days 30` 真实回测通过, 10 片全部发出并成交, 无报错 ✅
- SC-003: 旧策略测试(dca/twap/pullback/ladder/shannon_grid)74 个全过, volume 字段向后兼容 ✅

## 变更档案

- 代码: `strategies/builtin/executors/vwap.lua`(新增); `lua.rs`(+volume 字段); `mod.rs`(+BUILTIN_SCRIPTS 行); `backtest.rs`/`run.rs`(+inline_config vwap 分支); `builtin_tests.rs`(+3 用例 + kline_v helper)
- 文档(living spec 演进): specs/product.md / specs/lua-api.md / specs/architecture.md / README 两版 同步 vwap

## 备注

- 试点验证了 SDD 全流程可用: 中文产物模板生效(zh preset)、constitution 约束生效、converge 收敛闭环工作正常
- 无遗留任务; 本变更未触碰任何既有策略行为
