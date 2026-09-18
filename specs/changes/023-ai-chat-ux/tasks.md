# 023 任务分解

> 状态: **已完成** (2026-09-18) | 基线: bin 165 passed / 0 failed / 1 ignored · integration 3 passed / 0 failed / 9 ignored · clippy 0 警告

- [x] **T001** Step 0: SDD 变更档案 `specs/changes/023-ai-chat-ux/`(本目录三件套)
- [x] **T002** Step 1: 新建 `i18n.rs`(`Lang`/`parse`/`code`/`resolve`/`t`) + `main.rs` 登记
- [x] **T003** Step 2: `config_file.rs` 增 `[ui]` 段(`UiSection`/`File.ui`/`UI_KEYS`/`load` 校验/两处报错文案/`WRITABLE`/模板)
- [x] **T004** Step 3: `commands/mod.rs` 增 `require_simple_confirmation` / `update_strategy_params_in` / `delete_strategy_files_in`; 终端提示语跟随语言
- [x] **T005** Step 4: `confirm.rs` `ActionKind` 扩 13 变体 + 双语 `label()` + 13 构造器 + `params` 字段 + 口语确认/拒绝词表 + `classify_user_line(line,pending,lang)`
- [x] **T006** Step 5: `tools.rs` `VIRTUAL_TOOLS` 收敛为 3 + `show_menu` + `MenuSlot` + 删 `start_dry_run`/`stop_run` + `prepare_*` 增删改 + 文案双语化
- [x] **T007** Step 6: `session.rs` 轮次分隔/`/history`/菜单序号/`/lang`/`execute` 扩展/`execute_confirmed` 增 5 分支/重写 help 与 welcome
- [x] **T008** Step 7: `prompt.rs` `system_preamble(lang)` + 语言与表述纪律硬约束(保持 slim)
- [x] **T009** Step 8: `onboard.rs` `Gaps.lang` + 向导首问语言 + 提示语跟随语言
- [x] **T010** Step 9: `chat.rs` 提示符/退出语/欢迎流程双语
- [x] **T011** Step 10: 终端门禁零改动(仅补注释)
- [x] **T012** Step 11: 测试改写与新增(漂移闸 / 确认语义 / 菜单 / 双语 / 新增写能力)
- [x] **T013** Step 11: 文档同步 + 编译/clippy/单测验证 + demo 真实调用 + `converge.md`

## 实施记录

**门禁(本次实跑, 2026-09-18 Windows)**: `cargo fmt --all -- --check` → `FMT_EXIT=0`; `cargo clippy -p ricow --all-targets -- -D warnings` → `CLIPPY_EXIT=0`; `cargo build -p ricow` → `BUILD_EXIT=0`; `cargo test -p ricow` → `TEST_EXIT=0`。bin `166 tests → 165 passed / 0 failed / 1 ignored`; integration `12 tests → 3 passed / 0 failed / 9 ignored`。

**实测修复(为达成 clippy 零警告)**: 5 处 —— `ai/session.rs` `needless_borrow`(L179)、`useless_format`(L364); `commands/onboard.rs` `question_mark`(L115); `ai/tools.rs` `unused_variables`(测试辅助 `deploy_via_confirmation` 删 `name` 形参 + 9 处调用点)、`bool_comparison`(L2259 改 `!is_explicit_confirmation(...)`)。随后 `cargo fmt --all` 并**重跑整条门禁链**取得单一一致记录。

**SC-008 真实 demo 闭环(宪法三, 禁 mock)**: 以终端渠道 + 与对话内 `StartDemo`/`StopDemo` **同一批 `ctrl` 内核**在真实币安 demo(`https://demo-api.binance.com`)取证 —— `orderId=65828939573`(`BTCUSDT`, `status=NEW`, `price=78523.67000000`)→ `ricow stop sc008ladder`(`exit=0`, 2107ms)→ 引擎日志 `cancelled=1 cancel_failed=0 residual_orders=0` → 外部签名查询 `openOrders` **1 → 0**(三向一致)。**边界**: 对话内 tty REPL 端到端受 D5 tty 门禁约束, agent shell 非 tty 无法驱动, 归人工项。

**未跑(如实, 不以 mock 顶替)**: ① 真 tty 双语手工冒烟(plan 动作 4); ② 对话内确认的真机端到端; ③ `#[ignore]` 真机 S1–S13(需 `RICOW_AI_API_KEY` 与公网)。

**详细 FR/SC 对照与口径说明**: 见同目录 [`converge.md`](./converge.md)。

