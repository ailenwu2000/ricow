# 实施计划: VWAP 执行模式示例

**分支**: `001-vwap` | **日期**: 2026-09-01 | **规格**: [spec.md](spec.md)

**输入**: `/specs/changes/001-vwap/spec.md` 的功能规格

**说明**: 本模板由 `/speckit-plan` 命令填充。

## 摘要

新增 `strategies/builtin/executors/vwap.lua` 执行模式示例(与 dca/twap/pullback/ladder 同级): 按时间片分批, 每片参考最近已收盘 K 线的成交量加权均价(VWAP)挂限价单。配套扩展 `ctx:klines` 暴露 `volume` 字段(向后兼容), 完成 CLI 直跑注册与回测冒烟测试。

## 技术上下文

**语言/版本**: Rust 1.83(workspace) + Lua 5.4(mlua 沙箱, 策略层)

**主要依赖**: 无新增依赖(复用 exec.* 组件与现有 ctx API)

**存储**: 不涉及(纯逻辑 + 已收盘 K 线缓存)

**测试**: `cargo test --workspace`(基线 151); 回测冒烟用 `locus backtest --strategy vwap --pair BNBUSDT --days 30`(binance 真实 K 线, 非 mock)

**目标平台**: Linux / WSL(CLI)

**项目类型**: 库(locus_strategy) + CLI(locus_cli) + Lua 资产(strategies/builtin/executors/)

**性能目标**: 无特殊要求(每 tick 最多 100 根 K 线的加权求和, 开销可忽略)

**约束**: 无前视(只用已收盘 K 线); 沙箱限制(无 os/io); 向后兼容 ctx:klines 既有字段

**规模/范围**: 1 个 Lua 资产 + 2 处 Rust 小改(klines 字段、CLI 注册)+ 测试

## 宪法检查

*门禁: 必须通过。*

- [x] 原则二(策略层统一 Lua + exec): vwap 是执行模式示例, 复用 exec.ticks_per / exec.slice_due, 符合
- [x] 原则三(测试纪律): 纯逻辑 + 真实 K 线回测冒烟, 无 mock 替身
- [x] 原则四(产物一律中文): spec/plan/tasks 全部中文
- [x] 原则五(少而精): 最小变更面, 不重构既有代码

## 项目结构

### 文档(本次功能)

```text
specs/changes/001-vwap/
├── plan.md              # 本文件
├── spec.md              # 功能规格
├── checklists/requirements.md  # 规格质量清单
└── tasks.md             # /speckit-tasks 输出
```

### 源代码(仓库根)

```text
strategies/builtin/executors/vwap.lua   # 新增: 执行模式示例
crates/locus_strategy/src/lua.rs        # 修改: ctx:klines 增加 volume 字段
crates/locus_strategy/src/builtin_tests.rs # 修改: 新增 vwap 回测冒烟用例
crates/locus_cli/src/commands/mod.rs    # 修改: BUILTIN_SCRIPTS 注册 vwap
crates/locus_cli/src/commands/backtest.rs # 修改: inline_config vwap 默认参数
crates/locus_cli/src/commands/run.rs    # 修改: inline_config vwap 默认参数
specs/lua-api.md                        # 修改: klines 文档加 volume
```

**结构决策**: 严格复用既有执行模式(dca/twap)的结构: Lua 资产在 executors/, 注册走 BUILTIN_SCRIPTS 常量表(路径集中一处), 参数透传走 inline_config——不引入新模式。

## 技术方案

### 1. ctx:klines 增加 volume(lua.rs:137 附近)

`row.set("volume", kline.volume.to_f64()...)`——与 open/close 同构, 向后兼容(旧策略只读 open/close 不受影响)。同步 specs/lua-api.md 的 klines 行说明。

### 2. vwap.lua 算法

```
参数: pair / total_size / num_slices / slice_interval_secs / lookback_bars(默认 20) / side(默认 buy)
on_init: 读参数; ticks_per_slice = exec.ticks_per(slice_interval_secs, bar_seconds)
on_tick: start_tick 首片; slice_due 到期 → 算 VWAP → 构造订单
VWAP: 遍历 ctx:klines(pair) 最近 lookback_bars 根(倒序取),
      Σ(close×volume)/Σ(volume); 无成交量(Σvolume==0)回退最新 close;
      K 线不足用可用根数
订单: {pair, side, size=total_size/num_slices, price=vwap, order_type="limit"}
      —— 与 exec.side_order 的对手价不同, vwap 用计算价限价, 故脚本内自行构造
on_fill: 记录成交(与 twap 一致, 仅日志)
```

### 3. CLI 注册

- mod.rs: BUILTIN_SCRIPTS 表加一行 `("vwap", include_str!(.../executors/vwap.lua))`
- backtest.rs / run.rs inline_config: `"vwap"` 分支默认参数(total_size=1.0, num_slices=10, slice_interval_secs=60)

### 4. 测试(builtin_tests.rs)

- include_str! 加载 vwap.lua; 构造 90 根合成 K 线(带 volume); 回测冒烟: 断言订单发出次数 = num_slices、首片立即、每片 size = total/num、无前视(用已收盘序列)

## 复杂度追踪

> 无宪法违规, 不填。
