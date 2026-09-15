---
description: "VWAP 执行模式实施任务清单"
---

# 任务: VWAP 执行模式示例

**输入**: `/specs/changes/001-vwap/` 设计文档

**前置条件**: plan.md(必填)、spec.md(用户故事必填)

**组织方式**: 任务按用户故事分组, 依赖顺序执行。

## 格式: `[ID] [P?] [故事] 描述`

- **[P]**: 可并行(不同文件, 无依赖)
- **[故事]**: 任务所属用户故事(US1 直跑 / US2 复制即自定义 / US3 成交量读取)

---

## 阶段 1: 地基 (US3 成交量能力 — 阻塞后续)

**目的**: ctx:klines 暴露 volume, VWAP 计算的前提

- [x] T001 [US3] 修改 `crates/locus_strategy/src/lua.rs`: klines 方法中每根 K 线增加 `row.set("volume", ...)`(与 open/close 同构, 向后兼容)
- [x] T002 [US3] 修改 `specs/lua-api.md`: klines 行说明增加 volume 字段

**检查点**: klines 返回 {open, close, volume}, 旧字段不变

---

## 阶段 2: 用户故事 1 - 直跑 VWAP 分批回测 (P1) 🎯 MVP

**目标**: --strategy vwap 直跑可用

- [x] T003 [US1] 创建 `strategies/builtin/executors/vwap.lua`(参数读取 / ticks_per 换算 / slice_due 节奏 / VWAP 计算 / 限价单构造 / 成交量缺失回退 close)
- [x] T004 [US1] 修改 `crates/locus_cli/src/commands/mod.rs`: BUILTIN_SCRIPTS 表加 `("vwap", ...)` 一行
- [x] T005 [US1] 修改 `crates/locus_cli/src/commands/backtest.rs` + `run.rs`: inline_config 加 `"vwap"` 分支(默认 total_size=1.0, num_slices=10, slice_interval_secs=60)

**检查点**: `locus backtest --strategy vwap --pair BNBUSDT --days 30` 可运行

---

## 阶段 3: 测试与打磨 (US1/US2)

- [x] T006 [US1] 修改 `crates/locus_strategy/src/builtin_tests.rs`: 新增 vwap 回测冒烟用例(合成 90 根带 volume 的 K 线; 断言首片立即、发出次数 = num_slices、每片 size = total/num、无前视)
- [x] T007 [P] 全量验证: `cargo test --workspace` 全过(≥151+); `cargo build` 无新增 warning

**检查点**: 所有用户故事独立可用; 旧策略(dca/twap/pullback/ladder/shannon_grid)测试不受影响

---

## 依赖与执行顺序

- T001/T002(US3)→ 阻塞 T003(VWAP 需要 volume)
- T003 → T004 → T005(注册链)
- T006 依赖 T001-T005 完成
- T007 最后全量验证

### 并行机会

- T001 与 T002 可并行(不同文件)
- 无其他并行(单文件依赖链)

## 备注

- 每任务完成后 cargo check 增量验证
- 提交纪律: 用户明确说"提交"才可 git commit
- 避免: 模糊任务、超范围重构(严格只改计划内文件)
