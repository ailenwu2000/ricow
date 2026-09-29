# 044 `ricow optimize` 参数优化 (竞品对标 P2)

## 背景

竞品对标 (`.trae/documents/competitor-gap-analysis.md`) P2: Freqtrade hyperopt 对标。纯 CLI 层, 无引擎改动 —— 复用 `backtest.rs` 现有的 `run_backtest` + `parse_param` + 三层参数覆盖; 多组 `--param` 网格/随机搜索 → 汇总打分表 → 最优参数写回 TOML (可选)。

用户口径不变: Web UI 不做, 风控不做。

## 需求

### FR-1 命令形态

```
ricow optimize --strategy <name> \
  --param spacing=0.004,0.006,0.008 \      # 逗号分隔多值 = 网格维度
  --param offsets=1,2,3 \
  [--days 90 | --start/--end] [--interval 1h] \
  [--search random --samples 20] \         # 缺省 grid; random 时每维在给定值中随机抽
  [--metric net_pnl|sharpe|annual_return]  # 排序指标, 缺省 net_pnl
  [--write-back]                           # 把最优组合写回 strategies/<name>.toml
```

- `--param` 每个键给逗号分隔候选值 (值解析复用 `parse_param` 的类型规则: true/false→Boolean, 数字→Float, 其余 String; 键多个候选 = 网格笛卡尔积)。
- 仅支持**已部署策略** (strategies/<name>.toml); 直跑模式不支持 (无参数基线可回填)。
- 组合数 = 各维候选数乘积; `--search random` 时抽 `--samples` (默认 20) 组, 有放回去重。

### FR-2 执行与打分

- 每组组合跑一次完整回测 (同一窗口/interval, K 线只拉一次复用)。
- 汇总表: 每行 = 参数组合 + `net_pnl` / `total_trades` / `max_drawdown` / `sharpe` / `win_rate`, 按 `--metric` 降序; 失败组合单独列出 (错误摘要, 不中断整批)。
- 打印最优行与对应参数键值。

### FR-3 写回 (可选)

- `--write-back`: 把最优组合的参数合并进 `strategies/<name>.toml` 的 `params` 表 (只改命中的键, 其余行/注释保留, 复用 `/keys` 静默录入的外科式写法同思路); 打印 diff 摘要。
- 无 `--write-back` 时只读不写。

### FR-4 成本护栏

- 组合数 > 100 时拒绝执行并提示收窄网格 (每组 = 一次全窗口回测)。
- 逐组进度日志 (tracing info: 第 i/N 组, 当前组合)。

## 非目标

- 贝叶斯/TPE 等智能搜索 (hyperopt 完全体); 过拟合检测; Web 展示。

## 验收

- 单测: 参数网格展开 (笛卡尔积/随机去重/类型解析), 打分表排序, TOML 写回只动命中键。
- 手验: 对 `shannon_demo2` 用 2×2 网格跑 `optimize`, 报告与逐次 `ricow backtest --param` 一致; `--write-back` 后 TOML diff 仅含最优键。
