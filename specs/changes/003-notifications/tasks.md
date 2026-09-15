# 003 任务分解

> 状态: **全部完成**(2026-09-13)。
> 基线: 289 → **294 passed / 0 failed / 11 ignored**。
> 规则: 纯逻辑单测; 投递用真实 HTTP 往返(本地端点)验证, 不 mock 网络层断言。

## 阶段 1 — 配置与纯逻辑

- [x] **T001 `[notify]` 配置**: `NotifyToml` + `StrategyConfig.notify`(默认关闭); 判据: 单测(解析/缺省/未知字段容忍)
- [x] **T002 事件与格式化**: `NotifyEvent` 四类 + `format_message`; 判据: 单测(每类含关键字段)
- [x] **T003 筛选与限速**: 白名单 + `min_interval_secs` + 丢弃计数; 判据: 单测(压制/放行/计数)
- [x] **T004 强平告警去重**: 同 (pair, 方向) 只发一次, 恢复后可再发; 判据: 单测

## 阶段 2 — 投递与接线

- [x] **T005 WebhookNotifier**: `POST` JSON(`chat_id` 可选)+ 5s 超时 + 失败只 warn; 判据: SC-003 本地回环真实往返
- [x] **T006 引擎接线**: 实盘/Dry Run 构造 notifier; 成交 / 熔断拒单 / 强平告警 / 停机残留四类事件; 判据: 单测 + 实测
- [x] **T007 失败隔离实测**: 端点不可达时循环照常(SC-004)

## 阶段 3 — 文档

- [x] **T008 文档同步**: product.md §三.4、architecture.md、roadmap.md、本档案 converge

---

## 实施记录 (2026-09-13)

**实现**:
- `locus_engine/src/notify.rs`(新): `EventKind`(四类)+ `NotifyEvent` + `fmt_num` 数值规整(去 f64 尾差)
  + `NotifyConfig::from_config`(params 通道, 未配 `notify_webhook` 即关闭)+ `Throttle`(白名单/限速/压制计数)
  + `Notifier`(去重 + spawn 投递 + 5s 超时 + 失败只 warn)
- `locus_strategy/src/context.rs`: `take_circuit_reject()` —— 熔断类拒单(`ConsecutiveLossHalt` / `PeakDrawdownHalt`)
  留痕, 引擎 take 后发通知; 其余风控拒单(限额/最小单/滑点)只留日志, 不打扰用户
- `locus_engine/src/command.rs`: `run_live` / `run_dry_run` 各构造 notifier; 四类事件接线
  (成交=主循环 + 停机吸干 + Dry Run 清理回调; 熔断=Rejected ack 处; 强平=`warn_near_liquidation`; 残留=清理结束后)
- 顺带清理: 删除 `futures.rs` 中确实无生产调用者的 `directional_position_from_risk`(测试已覆盖同语义, 该函数无引用)

**单测(289 → 294)**:
- `test_event_kind_parse_and_whitelist` / `test_render_contains_key_fields`(四类含关键字段 + 压制数如实附带 + 尾差规整)
- `test_throttle_suppresses_then_reports_dropped`(窗口内压制累计 / 过窗放行带 dropped / 类别互不影响 / 0 = 不限速)
- `test_notify_config_from_params`(未配即关 / trim / 白名单解析含无效项忽略 / 默认 5s / chat_id)
- `test_liq_dedup_until_cleared`(同 pair+方向只发一次 / 恢复后复位 / 另一方向独立)

**真实 HTTP 往返验证(SC-003/SC-004)**:
- 本地回环接收器(127.0.0.1:8899)+ 50x 合约实盘: 收到 4 条 POST —— 买入成交、接近强平(距离 1.47%)、平仓成交等,
  文本与事件字段一致; **发现并修复一处重复投递**(见下)
- 失败隔离(Dry Run + `http://127.0.0.1:1/unreachable`): 仅 `WARN notify: 通知投递超时 (5s)`, 策略照常 `tick=72 成交=1`
- 计数复验(Dry Run + 可用端点): 成交 1 笔 → 通知 1 条

**实施期发现并修复的缺陷**:
1. **重复投递**: 批量替换 on_fill 行时用了子串(16 空格缩进的匹配串命中了 24 空格缩进的 live 主循环那行),
   导致同一成交发两次通知(实测 db 记录 2 笔 / 通知 3 条)。已删除重复块并复验计数一致。
2. **数值尾差入文本**: 通知里出现 `0.0040000000000000000832667269`(Lua f64 → Decimal)—— 新增 `fmt_num`
   (round_dp(8) + normalize)统一规整; 下单路径不受影响(引擎本就按 step/tick 对齐, 日志保留原值便于诊断)。

**未做 / 边界**: 通知不保证送达(不重试不排队)、不参与交易决策; 未接真实第三方通道(用本地回环端点验证真实 HTTP 往返)。
