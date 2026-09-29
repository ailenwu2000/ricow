# 043 实施计划

## 改动点

1. **`crates/ricow/Cargo.toml`** + **`main.rs`**
   - tracing-subscriber 开 `json` feature (确认现有 feature 集, 只加不换)。
   - main.rs L84-91: 读 `RICOW_LOG_FORMAT` env, == "json" 时走 `fmt().json()` 分支 (with_writer stderr / EnvFilter 逻辑复用); 其余走现纯文本。
2. **`crates/ricow/src/commands/logs.rs`**
   - `LogsArgs` 增 `--level <String>` (缺省 None = 不过滤)。
   - 过滤函数: 对每行做级别匹配 —— 纯文本行含 ` DEBUG `/` INFO `/` WARN `/` ERROR ` 词元 (tracing fmt 默认在 target 前输出级别大写词), JSON 行匹配 `"level":"ERROR"`。级别比较含阈值语义: `--level WARN` 输出 WARN+ERROR (按级别数值)。
   - follow 循环复用同一过滤。
3. **`crates/ricow/src/commands/instances.rs`**
   - format_table 表头增两列 `最新成交` / `当日盈亏` (宽度 20/12)。
   - 每实例: `ricow_strategy::db` 打开 `ricow.db` (只读, 失败即 `-`), 取最新 FillRecord (ts→ISO, price×qty); 当日盈亏取 PnlSnapshotRecord 当日 UTC 最新一条的当日值 (若无当日快照则取最新累计盈亏, 列名照实显示口径, `-` 兜底)。
   - 打开 db 的路径: 复用 instances 现有数据根解析 (与 info/fills 命令同源, 避免口径分叉)。

## 测试

- logs.rs 单测: 级别阈值过滤 (纯文本行/JSON 行/无关行), 非法级别报错。
- instances.rs 单测: 表格行在无 db / 有 fill 两态下的列宽与 `-` 兜底。
- `cargo test -p ricow` 全绿 + clippy 0。

## 验证

- `RICOW_LOG_FORMAT=json ricow daemon start` + 跑 shannon_demo2 → logs/<name>.log 首行为 JSON。
- `ricow logs shannon_demo2 --level WARN` 与手工 grep 对照一致。
- `ricow status` 新列与 `ricow info <name>` 数值一致。
