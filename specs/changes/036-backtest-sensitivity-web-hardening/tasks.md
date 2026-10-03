# 036 任务与完成记录

## T001 回测敏感性(第四节)

- [x] 抽 `BacktestOutcome` / `run_backtest_inner`(报告 / run card / 敏感性三共用)
- [x] `--sensitivity` / `--sensitivity-fee` CLI(可省档位用默认阶梯)
- [x] `parse_ladder` 校验(非数字/负数/非有限/单档硬失败, 中文)
- [x] 定宽表格 + 每轴结论段(显示宽度对齐, 中文列头)
- [x] 敏感性不落 run card; 无开关时报告逐字不变
- [x] 单测 4 组: `test_parse_ladder_defaults_and_errors` / `test_axis_summary_steady_and_flip` /
      `test_fmt_bps_and_display_width` / `test_sensitivity_table_includes_axes_and_rows`

## T002 SSE 续传 + 统一打开器(第五节)

- [x] `logs.rs`: 事件 id = 文件 offset; `Last-Event-ID` 头优先, `last_event_id` 查询参数兜底
- [x] `common.js`: `R.sse`(指数退避 1s→8s + ±20% 抖动, 连续 5 次放弃, `lastEventId` 重开续传)
- [x] `chat.js` 会话流 / 日志面板, `runs.js` 行内日志改接 `R.sse`; `new EventSource` 收敛到 common.js 一处
- [x] 测试: `test_log_stream_resumes_from_last_event_id_without_duplicates`

## T003 来源门(第五节)

- [x] `auth.rs`: `require_token` = token 门 + 来源门(先 token 后来源; 写方法限回环; 403 空体)
- [x] `ct_eq` 迁 auth.rs 并 `pub(super)`(runs 风险确认短语复用)
- [x] 测试: `test_is_loopback_origin_accepts_only_loopback` / `test_origin_gate_blocks_cross_site_writes_only`

## T004 Web 拆文件(第五节, 纯重构)

- [x] mod.rs 2178 → ~700; 7 个端点族模块各自带 `routes()` 与测试
- [x] `test_support.rs` 下沉共享脚手架(裸 HTTP / 临时目录 / 种数据)
- [x] `strategy_io` / `runs` 目录化, 测试体下沉 `tests.rs`
- [x] 路由装配与中间件语义不变; 既有用例断言不改(仅迁移位置)

## T005 验证与同步

- [x] fmt 0 差异 / clippy 零代码告警 / 安全红线门禁全绿
- [x] `cargo test --workspace --no-fail-fast` 全绿(见 converge)
- [x] 同步 `specs/backtest.md`(§十三 敏感性) / `specs/architecture.md`(web 模块布局 / SSE 续传 / 来源门) / `specs/roadmap.md`(基线)
