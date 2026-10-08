# 042 IM 告警增强: 平台适配 + 事件覆盖面 + 心跳 + 重试

## 背景

竞品对标 (`.trae/documents/competitor-gap-analysis.md`, 2026-09-29) 结论: 告警是差距最大、成本最低的改进项。003 已实现通用 webhook 通知 (fill/liq_warn/residual), 本变更在其骨架上扩展。用户已定口径: Web UI 不做, 风控不做 (策略自负)。

## 需求

### FR-1 平台适配
- `notify_format` 参数: `auto`(默认)/`telegram`/`feishu`/`slack`/`raw`。
- auto 按 URL 识别: 含 `open.feishu.cn` → 飞书; `api.telegram.org` → Telegram; `hooks.slack.com` → Slack; 其余 raw (现行为)。
- 请求体:
  - Telegram: `{"chat_id":?, "text":...}` (sendMessage 兼容)
  - 飞书: `{"msg_type":"text","content":{"text":...}}`
  - Slack: `{"text":...}`
  - raw: `{"text":..., "chat_id":?}` (不变)

### FR-2 事件覆盖面
新增 6 类事件 (白名单 `notify_events` 可选, 缺省全开):
- `started`: 实盘/demo 启动 (模式 + 交易对)。
- `stopped`: 停机 (原因 + 累计成交数)。
- `halted`: 策略置停机标记 (Lua `fatal=1`/`halted=1` 或 state 同名键) —— 每进程至多一条。
- `stall`: 策略停摆计数 `stat_stall_bars` ≥ `notify_stall_bars`(默认 1440) —— 每进程至多一条。
- `heartbeat`: 运行摘要, 每 `notify_heartbeat_secs`(默认 86400, 0=关) 一次: 运行时长/成交数/tick 数。
- `crashed`: 子进程自行退出 (supervisor 监控, 退出码)。

### FR-3 投递重试
- dispatch 失败 (网络错误/超时/非 2xx) 重试至 2 次, 退避 2s/8s; 仍在 spawn 任务内, 绝不阻塞交易循环 (003 FR-5 语义不变)。

## 非目标
- IM 双向控制 (启停指令), 策略市场, 风控引擎, Web 面板。

## 验收
- 单测: 事件渲染 / 白名单 / auto 格式识别 / 各平台 payload 形态 / halted/stall Lua 全局与 state 读取。
- testnet (shannon_demo2): 实收 started / fill / heartbeat 消息。
