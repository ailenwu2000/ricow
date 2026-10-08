# 042 实施计划

## 改动点

1. **`crates/ricow_engine/src/notify.rs`**
   - `EventKind` 增 `Started/Stopped/Halted/Stall/Heartbeat/Crashed`; `ALL_EVENTS` 扩至 9。
   - `NotifyEvent` 对应 6 个新变体 + `render` 文案。
   - `NotifyFormat` 枚举 (Auto/Telegram/Feishu/Slack/Raw), `notify_format` 参数, auto 按 URL 识别; `payload(format, chat_id, text)` 纯函数构建请求体。
   - `dispatch`: 最多 3 次尝试, 退避 2s/8s。
   - `notify_once(kind, ev)`: once 去重 (Halted/Stall/Crashed 每进程至多一条)。
2. **`crates/ricow_strategy/src/strategy.rs`** trait 默认方法: `halted()->bool` (false), `stall_bars()->Option<u64>` (None)。
3. **`crates/ricow_strategy/src/lua.rs`** `impl Strategy for LuaStrategy`: halted 查 Lua 全局 `fatal`/`halted` ≥1 及 `_RICOW_STATE` 同名键="1"; stall_bars 查 `_RICOW_STATE["stat_stall_bars"]` (字符串) 回退全局。
4. **`crates/ricow_engine/src/command.rs`** `run_live`:
   - 启动日志后 notify `Started{mode.label(), pair}`。
   - 新增 `summary_tick` interval (`notify_heartbeat_secs`, 默认 86400, 0=关) + `LiveEvent::SummaryTick` 分支 → `Heartbeat{uptime, fills, ticks}`。
   - `live_decision_tick` 末尾: `strategy.halted()` → notify_once Halted; `stall_bars() ≥ notify_stall_bars`(默认 1440) → notify_once Stall (阈值经参数传入)。
   - 末尾 "live run stopped" 后 notify `Stopped{reason: stop_reason/last_error, fills}`。
5. **`crates/ricow/src/supervisor/server.rs`** `monitor_loop`: 子进程自行退出 → `load_strategy_toml` → `Notifier::from_config` → notify `Crashed{exit_code, mode}`。

## 测试
- notify.rs 单测: parse/whitelist 9 类、新事件渲染、payload 四形态、auto 识别。
- ricow_strategy: 内联 Lua 验证 halted()/stall_bars()。
- `cargo test -p ricow_engine -p ricow_strategy -p ricow` 全绿。

## 验证
testnet `shannon_demo2` 实收 started/heartbeat; kill 子进程收 crashed。
