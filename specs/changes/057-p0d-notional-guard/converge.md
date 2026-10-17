# 057 收敛记录

**变更**: `specs/changes/057-p0d-notional-guard/`
**收敛日期**: 2026-10-18

## 一、交付内容

### P0-D 单笔名义上限
| 文件 | 改动 |
|:--|:--|
| `crates/ricow_strategy/src/order_guard.rs` | `NOTIONAL_LIMIT_MULTIPLE=10` / `check_notional()` 纯函数 / `quote_asset_of()` / `guard_check()`(频率→名义) / `OrderGuardError::NotionalLimitExceeded` |
| `crates/ricow_strategy/src/context.rs` | Live + DryRun 两处 `guard_reject` 接入，各自加 `equity_quote_balance` |

### G4 跨实例 kill switch
| 文件 | 改动 |
|:--|:--|
| `crates/ricow/src/supervisor/proto.rs` | `Request::StopAll { close_all }`（线上 `cmd=stop_all`） |
| `crates/ricow/src/supervisor/server.rs` | `stop_all_instances()` —— **逐个走 `self.stop`** |
| `crates/ricow/src/commands/ctrl.rs` | `KillArgs` / `kill()`(逐字短语) / `kill_daemon()` / `kill_report_text()` |
| `crates/ricow/src/main.rs` | `ricow kill [--close-all]` |
| `crates/ricow/src/web/runs/mod.rs` + `mod.rs` | `POST /api/runs/kill` + `KillAllReply` |

### G6 前端检查进 CI
| 文件 | 改动 |
|:--|:--|
| `.github/workflows/ci.yml` | 新增 `frontend` job（node 22 + `web_frontend_check.mjs`） |
| `.github/workflows/nightly.yml` | **新建**：CDP 真机走查（headless Chrome + 托管服务） |
| `scripts/ci_grep_gates.sh` | 头部注释更新（"故意不进 CI"已成历史） |

## 二、验收结果

| 项 | 结果 |
|:--|:--|
| `cargo test --workspace --no-fail-fast` | ✅ 见下方基线 |
| `bash scripts/ci_grep_gates.sh` | ✅ 五条红线全绿 |
| `cargo fmt --all -- --check` | ✅ 0 差异 |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ |
| `node scripts/web_frontend_check.mjs` | ✅ 七段全绿、exit=0（**真机验证过这条命令在 CI 里可用**） |
| `e2e_web.py`（干净 root + 无 AI 密钥 + `show_all_pairs=true`） | ✅ **458 passed / 0 failed**（与 054 基线完全一致，零回归） |
| CDP 真机走查 | ✅ **全绿**（唯一 NOTE 仍是 `/favicon.ico`，051 起遗留，不算缺陷） |
| **新增端点真机手测** | ✅ 见下 |

### 新增端点真机手测（curl 实测，非仅单测）
```
POST /api/runs/kill  {}                              → 400 confirm_required
POST /api/runs/kill  {"confirm":"y"}                 → 400 confirm_required   ← 裸 y 不认
POST /api/runs/kill  {"confirm":"确认停止全部"}        → 放行(随后因 daemon 未起诚实报错)
```
**确认纪律在真机上生效**：缺确认 / 错确认都被挡，只有逐字短语能过。

## 三、真机验证这一轮踩的坑（都是环境，不是代码）

按 memory 的纪律「成串 FAIL 先证伪再动手」，本轮两次成串 FAIL 都证伪为环境：

1. **第一轮 `got=0`（连不上）** → 判服务中途退出。根因：我用 `&` 后台起服务，父 shell 结束时被连带回收。改用 `run_in_background` 托管后正常。
2. **第二轮 `got=401`（鉴权不过）** → 先证伪：curl 用 Bearer 与 query 都是 200，**token 本身有效**。真因是我把 base URL 写成 `http://.../?token=xxx`，而脚本内部会再拼 `?token=`，**双 token** —— 而 query 优先 cookie 且不判空（043 修复的既定语义），第二个空值盖掉了真 token。改传纯 base 即通。
3. **第三轮 9 条 FAIL** → 同一根因：`show_all_pairs=false` 视野问题（回测段要 ETHUSDT，默认视野只有股票类）。memory 里记过这坑，改配置 + 重启后 458 全绿。

**一条新增教训**：`show_all_pairs` 是**启动期**读取，改配置必须重启服务；且配置是新 root 首次启动才生成的 —— 正确顺序是「起一次生成配置 → 改 → 同 root 重启」。

## 四、刻意不做（防下一个人当 bug 修）

1. **不做**最大持仓 / 单日亏损 / 回撤熔断 —— 属**投资判断**，继续由策略用 `ctx:net_pnl()` / `ctx:equity()` 自管（宪法 D15/D17）。拒单文案自带这条边界声明，单测锁住
2. **不做**可配置风险参数面 —— 一旦打开，宪法边界实质失效。倍数固定 10、不落配置
3. **权益取不到就放行**，不猜 —— 没有权益基准时拒单会把正常下单误拒（比不拦更坏）。宁可漏拦一次数量级笔误
4. **不做** kill switch 的前端按钮 —— 后端已就绪，UI 另立（避免在只读红线的页面里塞交易写动作）
5. **不做**把 kill switch 注册为 AI 工具 —— 宪法「写实动作不作工具注册」
6. **不做** daemon 侧批量强杀 —— 与既有「不静默强杀」纪律冲突
7. **不做**并发停机 —— 串行保证「逐个确定」，中断时不会有一批悬在中间
8. **不做** CDP 走查进主 CI —— 需真浏览器且偶发受 runner 资源影响，进 nightly；静态门进主 CI

## 五、后续

- kill switch 前端按钮 + P0-D 拒单的前端展示
- G2 下单改异步（`context.rs` 的 `block_in_place + block_on` 阻塞事件循环）
- G5 单交易对三层绑定、G7 超限文件拆分
- P4 dogfood 实盘验证（阻塞项 = **由用户执行**）