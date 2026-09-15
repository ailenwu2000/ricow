# 功能规格: VWAP 执行模式示例

**功能目录**: `specs/changes/001-vwap`

**创建日期**: 2026-09-01

**状态**: ✅ 已实施并收敛 (2026-09-01; 见同目录 converge.md)

**输入**: 用户描述: "新增 vwap 执行模式示例: 按时间片分批, 每片参考成交量加权均价(VWAP)挂限价单; 需要 ctx:klines 暴露成交量数据"

## 用户场景与测试 *(必填)*

### 用户故事 1 - 直跑 VWAP 分批回测 (优先级: P1)

用户执行 `locus backtest --strategy vwap --pair BNBUSDT`, 回测中按时间片分批买入/卖出, 每片参考最近已收盘 K 线的成交量加权均价(VWAP)挂限价单, 无盘口/无成交量数据时回退市价, 全程不报错。

**优先级理由**: 直跑是执行模式示例的基础能力(dca/twap 同级), 也是验收主线。

**独立测试**: 通过 `locus backtest --strategy vwap --pair BNBUSDT --days 30` 回测冒烟, 报告正常输出且成交/挂单记录符合分片数。

**验收场景**:

1. **给定** 90 天 BNBUSDT 1h K 线, **当** 运行 vwap 直跑回测(num_slices=10), **则** 产生 ≤10 次订单发出且无报错
2. **给定** K 线不足 lookback_bars, **当** 计算 VWAP, **则** 用可用根数计算, 不崩溃

---

### 用户故事 2 - 复制即自定义 (优先级: P2)

策略作者复制 `strategies/builtin/executors/vwap.lua` 到自己的 `strategies/scripts/`, 修改分片/窗口参数或信号逻辑, 形成自己的策略。

**优先级理由**: 执行模式示例的定位是"复制即自定义", 与 dca/twap/pullback/ladder 一致。

**独立测试**: 复制文件并改 `num_slices` 参数后回测仍可运行。

**验收场景**:

1. **给定** 复制的 vwap.lua, **当** 修改 num_slices 并回测, **则** 分片行为按新参数生效

---

### 用户故事 3 - Lua 策略读取成交量 (优先级: P3)

自定义 Lua 策略作者调用 `ctx:klines(pair)`, 每根已收盘 K 线除 open/close 外还能读到 `volume`, 用于成交量类信号(如放量确认)。

**优先级理由**: VWAP 依赖成交量, 是本次变更的基础能力; 对旧策略向后兼容。

**独立测试**: 在 Lua 脚本中读取 volume 字段并断言数值 > 0。

**验收场景**:

1. **给定** 任意已收盘 K 线缓存, **当** 调用 `ctx:klines(pair)[1].volume`, **则** 返回该 K 线真实成交量(f64), 旧字段 open/close 不变

---

### 边界情况

- K 线根数不足 `lookback_bars` → 用可用根数计算 VWAP
- 全部 K 线无成交量(volume 全 0)→ 回退最新 close 价挂单
- `slice_interval_secs` / `bar_seconds` 换算走 `exec.ticks_per` 守卫(≤0 返回 1)
- 回测中限价单价格未触及 → 允许不成交(与 ladder 一致, 只断言订单发出)

## 功能需求 *(必填)*

### 功能需求

- **FR-001**: 系统必须支持 `--strategy vwap` 直跑, 与 dca/twap 同级(内置脚本直跑模式)
- **FR-002**: vwap 必须按 `slice_interval_secs` 分片, 每片发出 `total_size / num_slices` 数量的订单(首片立即, 之后每片间隔)
- **FR-003**: 每片参考价必须为最近 `lookback_bars` 根已收盘 K 线的成交量加权均价(VWAP = Σ(close×volume)/Σ(volume)), 无前视
- **FR-004**: 系统必须支持限价单挂 VWAP 参考价; 成交量数据不可用时回退市价
- **FR-005**: `ctx:klines` 返回的每根 K 线必须增加 `volume` 字段, 且 `open`/`close` 保持向后兼容
- **FR-006**: 参数校验: `num_slices ≥ 1`, `total_size > 0`, `lookback_bars ≥ 1`(默认 20), `side` 默认 buy

### 关键实体 *(涉及数据时填写)*

- **已收盘 K 线(Kline)**: 交易所返回的 OHLCV; Lua 侧暴露 open/close/volume(本次新增), 最多最近 100 根, 无前视

## 成功标准 *(必填)*

### 可度量结果

- **SC-001**: `cargo test --workspace` 全过, 测试数 ≥ 151(基线)+ 新增 vwap 回测冒烟用例
- **SC-002**: `locus backtest --strategy vwap --pair BNBUSDT --days 30` 回测冒烟通过, 订单发出次数 = num_slices, 无报错
- **SC-003**: 旧策略(dca/twap/pullback/ladder/shannon_grid)回测冒烟不受影响(volume 字段向后兼容)

## 假设

- 假设 A1: VWAP 基于最近已收盘 K 线计算, 不含当前未收盘 bar(无前视纪律)
- 假设 A2: 无成交量数据时回退最新 close(先保证可运行, 不做复杂插值)
- 假设 A3: 限价单成交与否由撮合决定, 执行模式只保证"按节奏发出订单"(与 twap/ladder 一致)
- 假设 A4: 本变更为"执行模式示例", 不进入产品定位的"策略"范畴(见 specs/constitution.md 原则二)
