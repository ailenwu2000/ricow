# 039 收敛记录

## 实跑验证(2026-10-06, Windows, agent 沙箱)

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 0 差异 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 零**代码**告警(17 条 warning 全为 Windows 增量编译锁文件 `os error 5`: 17 个 target 各报 "generated 1 warning" 且详细行只有锁文件那一条) |
| `scripts/ci_grep_gates.sh` | 5 条红线全绿(含 Instant 无裸减法) |
| `cargo test --workspace --locked --no-fail-fast` | **812 passed / 0 failed / 22 ignored** |
| 前端语法 | 9 份 JS 全部 `node --check` 通过 |
| 内置 LWC v4.2.3 API 面 | `addAreaSeries` / `addHistogramSeries` / `priceFormat.type ∈ {custom, volume}` / `scaleMargins` / `priceScaleId` / `setMarkers` 均在包内(grep 确认) |
| web 真机冒烟(官方 `e2e_web.py`) | **144 PASS / 0 FAIL**(exit 0) |
| 图表载荷探针(`tmp/probe_039_chart.py`) | **14/14 PASS** |

**基线增量**: 806 → 812(**+6**) = ricow bin 407 → 411(基准序列 2 + 回撤序列 1 + `Option` 抽稀 1) + ricow_strategy lib 197 → 199(平仓明细与聚合一致 1 + 超限保留最近且总数不丢 1); 其余 target 一字未变。

## 真机取证细节(可复跑)

1. 起服务: `RICOW_ROOT=D:/sunhuazhu/ricow/tmp/verify-039 ./target/debug/ricow.exe web --no-open --port 18795`
   (**先 `cargo build`** —— 自己起的服务会锁 `target/debug/deps/ricow.exe`, 否则 LNK1104)。
2. 冒烟: `RICOW_E2E_ROOT=verify-039 python tmp/e2e_web_039.py http://127.0.0.1:18795 <token>` → 144 PASS / 0 FAIL。
3. 载荷探针: `python tmp/probe_039_chart.py http://127.0.0.1:18795 <token>` → 14/14 PASS, 实测:
   - `times/price/equity/benchmark/drawdown` 五条全 720(30 天 × 1h, 未触发抽稀);
   - `drawdown` 逐点 == 由载荷自身 equity 重算的 `(权益−滚动峰值)/峰值`, 且 `min == −max_drawdown_pct/100`;
   - `benchmark` 的 `None` 是**连续前缀**(建仓前 1 个点空、其后 719 个非空), `benchmark[i]/price[i]` 恒为常数(40.0411623…, 极差 < 2e-14);
   - 基准末点 104651.58, 反解本金 = **100000.0000 == metrics.initial_cash**(与指标卡 `benchmark_return_pct=4.6516%` 同源);
   - `Σ closed.pnl = 129.23849181877816 == metrics.realized_pnl`(逐位相等), 时间升序, `closed_total == 明细条数`。

## 收敛期踩到的坑(供下次跑冒烟复用)

1. **冒烟脚本的 root 解析依赖脚本自身位置**: `e2e_web.py` 里 `ROOT = <脚本同级目录>/<RICOW_E2E_ROOT>`,
   而 skill 现在装在 `~/.workbuddy/skills/` 下 —— 直接跑会把"落盘布局核对"与 `zz_dup_probe` 探针的
   读写全指向 skill 目录, 于是那批断言**全红且看起来像产品缺陷**(实测 31 条 FAIL)。
   **正确姿势**: 把脚本复制到仓库 `tmp/` 下再跑(`cp e2e_web.py tmp/e2e_web_039.py`),
   `RICOW_E2E_ROOT` 才会解析到 `<repo>/tmp/<名字>`。
2. **临时 root 的 `show_all_pairs=false` 会在回测段拦下 ETHUSDT**(报错文案点明"不在当前视野") ——
   该段是本轮**最该跑到的部分**(图表载荷就在回测响应里)。先
   `POST /api/config/keys {"updates":[{"section":"market","key":"show_all_pairs","value_bool":true}]}` 再跑。
3. **探针自身的公式要先自检**: 第一次跑出 1 条 FAIL, 查下来是探针把 `基准末点/结构常数`(等于价格)
   当成收益率 —— 产品侧无问题, 改探针为"反解本金 == initial_cash"后 14/14。**结论: 断言失败先分清
   是产品还是断言**。
4. 重跑冒烟前必须清 `RICOW_ROOT/strategies/zz_web_probe.*` 与 `*.bak`(否则新建步骤变 409)。

## 决策记录

- **不做开→平 round-trip 配对**(D3): 合约对冲 / 组合模式下"这笔卖是开空还是平多"无唯一解;
  引擎本来就是"平仓事件"语义(`record_pnl` 一次 = 一次平仓), 沿用它才能让明细与胜率/已实现盈亏**机械自洽**。
- **派生序列一律后端全分辨率计算再抽稀**(D2): 前端只有抽稀后的序列, 对抽稀序列重算回撤会系统性低估、
  基准起点会漂 —— 这不是性能取舍, 是口径正确性。
- **K 线涨跌配色修正**: 既有 `renderChart` 用 `--accent`(绿)作涨色, 与项目自身约定(指标卡/历史表 → `--error` 红)
  相反; 本轮改在同一个函数里做, 故一并修正(属本轮范围内的正确性修复, 非顺手重构)。
- **指标固定 MA7/25/99 + 成交量常显**(D1): 不做开关面板与可配参数(YAGNI), 需要时再加。

## 遗留

- **浏览器像素级渲染未截图取证**(本机未装 agent-browser, 装 Chromium 约 500MB 且与本仓"零构建链"无关)。
  已用替代手段覆盖: 前端语法 `node --check` + 内置 LWC v4.2.3 的 API 面核对 + 载荷数值 14 项交叉断言。
  如需像素级证据, 装 agent-browser 后按 skill 走一遍即可。
- **逐笔盈亏的"交易级"归并**(开→平配成一条交易、算持仓时长与单笔收益率)仍是后面的事 ——
  需要引擎侧成交流水与配对的显式契约, 属新变更。
- **行情实时化(K 线/盘口增量推送)**: 用户已定路线「后端 WS → SSE」, 另起 040。
