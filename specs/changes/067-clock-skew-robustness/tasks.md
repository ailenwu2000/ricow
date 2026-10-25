# 067 任务清单

状态：✅ 全部完成（2026-10-25）

## T1 时钟预检：多组采样 + 取 RTT 最小者

- ✅ T1.1 纯逻辑落 `ricow_engine::live`：`ClockSample` / `pick_clock_sample` /
  `clock_samples_agree` / `CLOCK_SAMPLE_AGREE_MS`
- ✅ T1.2 `pick_clock_sample` 单测：按 RTT 选（不按"偏差看起来最合理"）、空输入 → `None`
- ✅ T1.3 `clock_samples_agree` 单测：边界（差 500 视为一致）
- ✅ T1.4 `commands::mod.rs::fetch_clock_skew` 改多组采样（最多 3 组，两组一致提前收手）
- ✅ T1.5 拆出 `fetch_server_time(market, mode)`：现货/合约/demo 端点各取各的
- ✅ T1.6 采样日志回显（`rtts_ms` / `skews_ms`）
- ✅ T1.7 `ricow_engine/src/lib.rs` 导出新符号

## T2 demo 真机走查固化成 `scripts/demo_walk.py`

- ✅ T2.1 自举写探针策略（目录清单 + **根目录实例配置**两处都写）
- ✅ T2.2 探针策略**真下单**（首个 tick 提交一张永不成交的限价买单）
- ✅ T2.3 启动前预检 demo REST（不可达 → exit 2，不误导成产品 FAIL）
- ✅ T2.4 幂等清理：先停掉可能遗留的同策略运行实例
- ✅ T2.5 启动重试（最多 3 次，容忍启动期网络抖动）
- ✅ T2.6 铁证：直接签名查询 demo `openOrders`
- ✅ T2.7 停机后二次查询确认撤干净
- ✅ T2.8 解析引擎账目（只扫本轮新增日志，`pre_len` 界）
- ✅ T2.9 浏览器异常：剥 ANSI 无关、区分 favicon/404/5xx、记录 ≥400 的 URL

## T3 修脚本自身的四类假红/假绿

- ✅ T3.1 签名 URL 跨重试复用 ⇒ 每次重试重新取时间戳重新签名
- ✅ T3.2 HTTPError 分支抓响应体（币安错误码只在 body 里）
- ✅ T3.3 读引擎日志先剥 ANSI 颜色码
- ✅ T3.4 只扫本轮新增日志（`pre_len`），不清空日志文件

## T4 门禁与基线

- ✅ T4.1 `cargo fmt`
- ✅ T4.2 `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- ✅ T4.3 `cargo test --workspace --no-fail-fast` → **1093 passed / 0 failed / 22 ignored**
- ✅ T4.4 `bash scripts/ci_grep_gates.sh` → 全绿
- ✅ T4.5 `node scripts/web_frontend_check.mjs` → 全绿
- ✅ T4.6 基线同步两处镜像（`specs/roadmap.md` + `specs/architecture.md` 第九节）

## T5 变更档案

- ✅ T5.1 `spec.md` / `tasks.md` / `converge.md` 齐
- ✅ T5.2 取号 067（目录 + 全仓引用最大值为 066，不复用）
