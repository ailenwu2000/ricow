# 067 实施计划

| 阶段 | 内容 | 依赖 |
| -- | -- | -- |
| P0 | 纯逻辑落 `ricow_engine::live`（采样挑选 + 一致性判定）+ 单测 | 无 |
| P1 | `commands::mod.rs::fetch_clock_skew` 改多组采样、拆 `fetch_server_time` | P0 |
| P2 | `ricow_engine/src/lib.rs` 导出新符号 | P0 |
| P3 | `scripts/demo_walk.py`：自举探针策略 + Web UI 全流程 + openOrders 铁证 | 无 |
| P4 | 修脚本自身四类假红/假绿（签名复用 / 错误体 / ANSI / 日志累积） | P3 |
| P5 | 验证（门禁六件 + demo 真机两轮复跑）+ 基线两处同步 + 档案 | P1–P4 |

## P0：纯逻辑（可单测）

**文件**：`crates/ricow_engine/src/live.rs`

- `ClockSample = (i64, i64)` —— `(往返耗时 ms, 偏差 ms)`。
- `pick_clock_sample(&[ClockSample]) -> Option<i64>` —— **按 RTT 最小**选那一组的偏差。
  - 判据必须是 RTT：偏差测量的误差**只来自往返不对称 `(B-A)/2`**，量级 ≤ 往返耗时 ⇒ RTT 最小者噪声最小。
  - **不能**改成"偏差看起来最合理"/"偏差绝对值最小" —— 那会选中一个恰好接近 0 的坏样本。
  - 空输入 → `None`（取不到 ≠ 0，调用方据此报错，不许回退默认值）。
- `clock_samples_agree(a, b, tol) -> bool` —— 两组够一致（差 ≤ `CLOCK_SAMPLE_AGREE_MS = 500`）即可提前收手。
- 单测：按 RTT 选 / 忽略偏差绝对值 / 空输入 / 单样本 / 一致性边界（含 500 的边界值）。

## P1：取数改多组采样

**文件**：`crates/ricow/src/commands/mod.rs`

- `fetch_clock_skew(market, mode)`：
  1. 最多采 **3 组**，每组 = `t0` → `server_time()` → `t1`，算 `rtt = t1 - t0`、
     `mid = t0 + rtt/2`、`skew = skew_ms(mid, server)`；
  2. **两组一致即提前收手**（省一次往返，不拖慢启动）；
  3. 取 `pick_clock_sample` 的结果；
  4. **一组都没成功** → 报"读取交易所服务器时间失败"（**不静默回退默认值**）。
- 拆出 `fetch_server_time(market, mode)`：现货/合约各取各的服务器时间；
  `mode == Demo` 时走 demo 端点（`DEMO_SPOT_URL` / `DEMO_FAPI_URL`）。
  现货与合约的 demo 服务器时间**实测差 1.5~1.9 s**，混用会白做校准。
- 日志回显采样：`时钟预检采样 (取 RTT 最小者) rtts_ms=[...] skews_ms=[...]` ——
  便于事后判断"是真钟差还是链路抖动"。

## P2：导出

**文件**：`crates/ricow_engine/src/lib.rs` —— `pub use live::{...}` 补
`pick_clock_sample` / `clock_samples_agree` / `ClockSample` / `CLOCK_SAMPLE_AGREE_MS`。

## P3：demo 真机走查脚本

**文件**：`scripts/demo_walk.py`（进主仓）

- **自举探针策略**（两处都写，缺一不可）：
  - `<root>/strategies/spot/<id>.toml`（目录清单：展示/搜索用，catalog **每次请求都重扫目录** ⇒ 改完不用重启 daemon）；
  - `<root>/strategies/<id>.toml`（**根目录实例配置：运行路径真正读的那份**，含 `script_path` / `pair`）。
    只写目录清单 ⇒ start 被早期门禁**静默拒**（`/api/runs` 返回 `[]`、磁盘无日志）。
- **探针策略要真下单**（占位策略只打日志 ⇒ 永远 `submitted=0`）：首个 tick 提交一张
  买一价下方 20% 的限价买单（名义 ~30 USDT，永不成交、停机撤单即干净）。
- 流程：预检 demo REST（不可达 → exit 2）→ 幂等清理遗留实例 → Web UI「新建实盘(demo)」
  → 等"运行中"→ 等待提交标记 → **对 demo 测试网签名查询 `openOrders` 拿铁证**
  → 勾选平仓停止 → 二次查询确认 `[]` → 解析引擎账目 → 浏览器异常分类。
- 退出码：`0` 全绿 / `1` 有 FAIL / `2` 环境故障。

## P4：修脚本自身的假红/假绿

| # | 坑 | 修法 |
| -- | -- | -- |
| 1 | 签名 URL 跨重试复用 ⇒ `timestamp` 过期 ⇒ `-1021`（HTTP 400） | `http_get_json` 收**可调用对象**，每次重试**重新取时间戳并重新签名** |
| 2 | 币安错误码只在响应体里 | HTTPError 分支 `e.read()` 抓 body |
| 3 | 引擎日志嵌 ANSI 颜色码 ⇒ 正则匹配不到 | 读日志先剥 ANSI |
| 4 | 日志跨轮累积 ⇒ 命中上轮旧标记 | 启动前记 `pre_len`，只扫本轮新增（**不删日志文件**：Windows 下 daemon 持句柄会锁） |
| 5 | 重构到共享 helper 时把 `X-MBX-APIKEY` 头丢了 | 头必须显式传；错误码/消息只在 body 里 |
| 6 | 浏览器 console 的 404/500 **不带 URL** | 挂 `page.on("response")` 记录 `status>=400` 的 URL；`favicon.ico` 404 单独识别、不计 FAIL |

## P5：验证与归档

- 门禁六件：`cargo fmt` / `cargo clippy --workspace --all-targets -- -D warnings` /
  `cargo test --workspace --no-fail-fast` / `ci_grep_gates.sh` / `web_frontend_check.mjs`。
- **demo 真机复跑两轮**（同一 root 连跑，验证脚本真能复跑而非跑通一次就作废）。
- 基线**整仓实跑读出**，同步两处镜像（`specs/roadmap.md` + `specs/architecture.md` 第九节）。
- 档案：`spec.md` / `plan.md` / `tasks.md` / `converge.md`。
