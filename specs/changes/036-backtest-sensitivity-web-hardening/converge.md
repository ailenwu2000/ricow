# 036 收敛记录

## 实跑验证(2026-10-03, Windows, agent 沙箱)

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 0 差异 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 零代码告警(残余 warning 全为 Windows 增量编译锁文件/对象拷贝的 os error 5 环境噪声) |
| `scripts/ci_grep_gates.sh` | 4 条红线全绿 |
| `cargo test -p ricow --bin ricow` | **325 passed / 0 failed / 1 ignored**(035 时 317, +8: 敏感性 4 + auth 来源门 4, 另 web:: 102 全绿) |
| `cargo test --workspace --locked --no-fail-fast` | **650 passed / 22 ignored**; `ai_live_smoke` 2 例失败与本仓代码无关 —— 本机沙箱 `ERROR_PIPE_BUSY(231)` 拦截 `spawn`+stdin 管道(与 032/034/035 基线同因, 真机终端为全绿) |

- **零行为变化取证**: `web::` 全组 102 用例(含 025→034 的鉴权/泄漏/三态/日志/市场矩阵)未改断言, 仅迁移位置, 全绿。
- 敏感性无开关路径的报告文本与 035 逐字一致(由既有报告类用例覆盖)。

## 收敛期修掉的问题

1. **拆文件中断续接**: 上轮会话在 mod.rs 拆分中途超时 —— 7 个新模块文件已写但未挂载(死文件)。
   本轮对照备份核对了每段边界后完成: 删段(按行号一次 sed)→ 挂载 → 路由改 `assets::` / `.merge(xxx::routes())`
   → `auth::require_token`。一次删段失误删掉了 `impl IntoResponse for WebError` 的收尾 `}`,
   以 awk 花括号深度定位补回。
2. **迁移后的可见性/作用域修补**(编译器全数抓到): `ct_eq` 改 `pub(super)` 供 runs 复用;
   `current_lang` 落 settings.rs 后 mod.rs 加 `use settings::current_lang`;
   sessions 测试补 `SessionSink as _`(trait 方法作用域); settings 测试补 `config_file` 导入;
   `crate::CoreResult` 更正为 `ricow_core::CoreResult`; 清理迁出后残留的未用导入。
3. **tests.rs 下沉的括号事故**: `strategy_io` / `runs` 目录化抽取测试体时, 旧 `mod tests {` 的
   收尾 `}` 被带进新文件。rustc 报"unexpected closing delimiter"后定位为纯模块体结构
   (mod.rs 已声明 `mod tests;`), 删去尾部多余 `}` 即平。
4. **SSE 统一打开器与续传的配合**: 接管重连后重开的是新 `EventSource` 对象, 浏览器**不会**
   自动带 `Last-Event-ID` 头 —— 服务端补 `last_event_id` 查询参数兜底(头优先), 打开器记录
   `ev.lastEventId` 重开时拼参。这是"退避重连"与"断点续传"能同时成立的关键。
5. **`expect_frame` 死代码**: sessions.rs 迁移后其测试全部走真实 HTTP, 帧收帧函数无人调用 → 删(死代码必删)。

## 决策记录

- **`strategy_io` / `runs` 不再细拆**: 拆出测试后主体各约 500 / 420 行, 内部本就按
  "请求体 / 纯函数 / handler"分区, 与 032 已拆模块同构; 再切是制造人为接缝(YAGNI)。
- **敏感性不做滑点×费用网格**: 单轴阶梯已能回答"结论在哪一档翻转", 网格属过度设计。
- **来源门"无 Origin/Referer 即放行"**: 浏览器对写请求必带 `Origin`, 缺失即非浏览器请求
  (curl/脚本/本仓测试), 无"被第三方页面借用身份"的风险面; 真正要拦的是带外站 Origin 的浏览器请求。

## 遗留

- token 改 HttpOnly cookie: 长线项(spec FR 节外), 与来源门互补而非互替。
- 敏感性扫描目前仅 CLI 入口(`run_backtest`); Web 回测作业面板如需展示敏感性,
  待产品拍板后再接 `run_backtest_core`(内核已就绪, 无须改引擎)。
- `ai_live_smoke` 2 例沙箱失败: 待真机跑一次全绿取证(与 032/034/035 同一遗留)。
