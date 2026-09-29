# 044 实施计划

## 改动点

1. **新文件 `crates/ricow/src/commands/optimize.rs`**
   - `OptimizeArgs` (clap): strategy / param(多值) / days / start / end / interval / search / samples / metric / write-back。
   - `expand_grid(params) -> Vec<Vec<(String, ConfigValue)>>`: 每键逗号切分 → `parse_param` 逐值解析 → 笛卡尔积; `random` 模式用已见集合去重抽 `samples` 组。
   - `run_one(config, combo, window) -> CoreResult<BacktestReport>`: 克隆基线 config, 逐键 `config.params.insert`, 复用 `backtest.rs` 的 `run_backtest` 路径 (K 线窗口复用由 `run_backtest` 内部拉取即可 —— 若其每次拉 K 线, 先保持简单逐组拉, 不为优化加缓存; 组合数护栏已限规模)。
   - 汇总: `ScoreRow { combo, net_pnl, trades, max_drawdown, sharpe, win_rate }`, 按 metric 降序打印; 失败行附错误。
   - `write_back(name, best, toml_path)`: 文本级外科替换 —— 定位 `[params]` 段内命中键行改值, 键不存在则段尾插入; 无 `[params]` 段则文件尾追加; 打印逐键旧→新。
   - main.rs 注册 `Optimize` 子命令。
2. **backtest.rs 微改**: `run_backtest` 与 `BacktestArgs` 已 `pub(crate)`, 若字段/构造不够用则补一个 `BacktestArgs::from_config` 式构造器 (保持现有 CLI 行为零改动)。

## 测试

- optimize.rs 单测: 网格笛卡尔积 (2×3=6 组), 随机去重 (samples ≤ 总量), Boolean/Float/String 值解析沿用 parse_param, `trim/排序/失败行`, write-back 文本替换三态 (命中改/段尾插/无段追加)。
- `cargo test -p ricow` 全绿 + clippy 0。

## 验证

- `ricow optimize --strategy shannon_demo2 --param spacing=0.004,0.006 --param offsets=1,2 --days 30` 输出 4 行打分表; 取一组合用 `ricow backtest --param ...` 复核 net_pnl 一致。
- `--write-back` 后 `git diff strategies/shannon_demo2.toml` 仅含最优键。
