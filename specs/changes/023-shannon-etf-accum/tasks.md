# 023 香农 ETF 指数增加策略 —— 任务清单

> 依据: `plan.md`(全文同 `.hermes/plans/2026-09-17_221854-shannon-etf-accum.md`)。
> 执行纪律: 逐任务做, 每任务跑完测试再进入下一任务; git 提交须用户明确说"提交"。
> 顺序依赖: Task 1→2→3→4→5 顺序; Task 6/7 可与 1-5 并行(改名/注册互不依赖); Task 8 依赖 2/4; Task 9→10→11→12→13 顺序(策略本体); Task 14 收尾; Task 15→16→17→18 顺序(模拟/实盘通道)。

- [x] **Task 1: 高周期重采样纯函数**
- [x] **Task 2: 高周期 ATR 通道(`Context::atr_tf`)**
- [x] **Task 3: Lua 暴露 `ctx:atr(pair, period, tf)`**
- [x] **Task 4: 单标的 K 线尾窗克隆修复(性能)**
- [x] **Task 5: 撤单指令(挂单可撤, 供重挂)**
- [x] **Task 6: 旧策略改名 `shannon_grid` → `shannon_rebalance`**
- [x] **Task 7: 新策略注册与 CLI 装配**
- [x] **Task 8: 回测报告增基准(满仓 + 敞口对齐)**
- [x] **Task 9: 骨架 + 纯函数 + 参数读取**
- [x] **Task 10: 建仓通道(唯一的 EMA 用途)**
- [x] **Task 11: 挂单成型 + 成交后重挂(策略主体)**
- [x] **Task 12: 守卫、计数与残余失衡**
- [x] **Task 13: 集成测试**
- [ ] **Task 14: 文档同步(含命名与契约)**
- [ ] **Task 15: K 线通道接线**
- [ ] **Task 16: 重启续接**
- [ ] **Task 17: Dry Run 观察与部署**
- [ ] **Task 18: demo 模拟盘真实调用验证**

## 进度记录

### Task 1: 高周期重采样纯函数 —— 完成(2026-09-17)
- 新增 `crates/ricow_strategy/src/multiframe.rs`: `resample_complete`(完整桶/缺口桶规则)、
  `tf_ms_of`、`TfAtrCache`(可见性无前视 + 桶内记忆); `lib.rs` 注册 `pub use`。
- 验证: `cargo test -p ricow_strategy --lib` 154 passed(148 基线 + 6 新增); clippy 0 告警; rustfmt 干净。
- 过程中修正两处真错: ①完整桶判据漏 `+1ms`(会把末根整点 bar 当半桶丢); ②测试断言 `low` 取错范围
  (应取桶内极值)。

### Task 2: 高周期 ATR 通道 + 回测装配 —— 完成(2026-09-17)
- `Context` 增 `atr_tf` / `set_tf_klines`(默认 no-op);`BacktestContext`(`tf_caches`, 无前视, 与
  `now_utc` 同源)、`DryRunContext`/`LiveContext`(`tf_cache: RwLock<..>`)三处实现。
- 装配: 引擎 runner 用**全段** klines 重采样一次预装;新增引擎级运行时参数 `warmup_bars`(预热段
  不进 tick 循环/报告);CLI 在声明 `atr_interval` 时多取 24h。
- 验证: `test_atr_tf_preloaded_visibility_and_cache` 通过;`cargo test --workspace` 全绿
  (ricow_strategy 155 passed);clippy 0 告警;6 个改动文件 rustfmt OK。
- 过程中修正: atr_tf 测试首跑红 = 我的断言 off-by-one(当前 bar 那一分钟未收盘 → 14:00 只能见
  0..13 桶);实现正确, 断言已改并写明原因。

### Task 3: Lua 暴露高周期 ATR —— 完成(2026-09-17)
- `lua.rs`: `LuaCtxData.tf_atr` + `fill_snapshot` 取值 + 注册 `ctx:atr_tf(pair)`;
  `specs/lua-api.md` 指标契约 + `warmup_bars` 运行时参数。
- 设计调整: 周期改**配置驱动**(`atr_interval`/`atr_period`), 不做 `ctx:atr` 第三参(理由见 plan Task 3)。
- 验证: `test_atr_tf_exposed_to_lua` 通过(未就绪 → Lua 侧 nil; 就绪 → 与独立重采样直算一致 1e-9)。

### Task 4: 单标的 K 线尾窗克隆修复 —— 完成
- `KLINES_TAIL = 100` 尾窗 + `#[ignore]` 性能对照用例: 3 万根历史下 1 万次 `ctx.klines` **44.5ms**,
  对照旧行为(每 tick 全量克隆)等效 **13.8s** → 约 310×;指标输入上限 100 根写入 `specs/lua-api.md`。

### Task 5: 撤单指令 —— 完成
- `OrderRequest.action`(Place/CancelPending);回测/模拟盘清挂单、实盘按归属前缀撤(`cancel_owned_orders`);
  Lua `{ pair=..., action="cancel_pending" }` 解析先于 size 守卫。测试: 撤单链路(清空/不计拒单/只撤指定 pair)
  + Lua 无 size 撤单指令可解析。
- 踩到的坑: `pending_orders` 元素是 `(订单号, 请求)`,按 pair 过滤必须看请求里的 pair(测试首跑红抓出)。

### Task 6: 旧策略改名 —— 完成
- `shannon_grid` → `shannon_rebalance`(21 个文件 62 处;历史档案 `specs/changes/**`、`specs/testnet.md`、
  `specs/research/**` 按宪法不回改);脚本文件同步改名(用 `mv` 而非 `git mv` —— 仓库存在 9/16 的陈旧
  `.git/index.lock`, 未动它)。

### Task 7: 新策略注册 —— 完成
- `BUILTIN_SCRIPTS` 注册 `shannon_etf_accum`;CLI 默认参数分支(backtest/run)新增
  `atr_interval/atr_period/atr_mult/ema_fast/ema_slow/target_ratio/min_notional/rehang_secs/fee_bps`。

### Task 8: 报告基准对照 —— 完成
- `BacktestReport` 新增 `benchmark_entry_price/entry_equity/strategy_return_since_entry_pct/
  benchmark_return_pct/benchmark_max_drawdown/benchmark_annual_volatility/
  benchmark_exposure_return_pct/benchmark_exposure_max_drawdown`;首笔成交**无条件**留痕;
  组合路径留 None(不编造);CLI 加"基准对照"小节(策略 / 满仓持有 / 敞口对齐 + 净贡献)。

### Task 9-13: 策略本体(建仓/挂单/重挂/守卫/测试) —— 完成
- `strategies/builtin/shannon_etf_accum.lua`:金叉建仓(EMA 仅建仓)、平衡价 = 末次成交价、
  上下 `atr_mult×ATR` 挂"精确回 target_ratio"的限价单、成交后先撤后挂、每小时按最新 ATR 重挂、
  守卫(保本线/最小名义/ATR 未就绪/现金兜底)、统计输出(成交/重挂/跳过计数)。
- 集成测试 5 例(通道未就绪不动 / 金叉建仓半仓 / 成交后先撤后挂两张且价差 2×spacing / 薄间距不挂 /
  小名义不挂);`cargo test --workspace` = 354 passed / 0 failed;clippy 0 告警。

### 回测(9 组 + 对照) —— 完成
- 结果与口径见 `backtest-2026-09-17.md`(窗口/建仓价实测、9 组对照表、三目标实测结论、守卫跳过计数)。

### 待办
- Task 14: 文档同步(`specs/product.md` 新策略规格 + `specs/backtest.md`/`roadmap`/`lua-api` 复核)。
- Task 15-18: 模拟盘 K 线刷新通道接线、重启续接(DB 注入)、Dry Run 部署观察、demo 真实调用验证。

---

## 2026-09-18 v3 修订进度(虚拟账本 / 最大区间 / 双通道)

方案: `.hermes/plans/2026-09-18_090500-qqqb-virtual-book-v3.md`;结果: `backtest-2026-09-18-virtual.md`

- [x] Task 1 虚拟账本 + 回平衡口径(`目标真实持仓 − 真实持仓`;修掉"清仓式卖出"与"卖量恒 0"两个真 bug)
- [x] Task 2 R1 初次金叉(锚 = ask+spacing,账本在锚价建 50:50,按当前价补到目标 ≈ 半仓)
- [x] Task 3 R2 金叉加仓(方向条件 `价 ≤ 锚−spacing`;实测:连续行情几乎不触发,由网格买单覆盖)
- [x] Task 4 R3 死叉卖出(真实仓位够→市价卖;不够→零下单 + 按 10 万整本重算账本;不变量 I1 + 单测)
- [x] Task 5 三通道同 tick 互斥 + 撤单优先(cancel 在 Place 之前)
- [x] Task 6 保本线参数化 `min_spacing_pct`(净零点 0.4%;实测与 0.2% 无显著差异,如实记录)
- [x] Task 7 逐笔状态行(持仓/权益/现金/锚/v_coin)+ on_stop 计数(交叉检出、跳过、只记账)
- [x] Task 8 回测口径:虚拟 10 万 1:1 落地;1 万账户自动 0.1 比例(实测精确 1/10 线性)
- [x] Task 9 六组回测(见结果文档):m2/m3/m4.5 + 旧保本线 + 关交叉 + 1 万账户;拒单 0,最大挂单 ≤2
- [ ] Task 10 文档同步:`specs/product.md` §十二(策略规格:虚拟账本口径、R1-R6、风险提示)待写
- 数据上限实测:QQQB 1m/5m/1h/1d 最早均 2026-06-30 → **6 个月不可得,取最大区间 79 天**(113,049 根)
- 尚未做(Task 15-18):Dry Run 部署、demo 真实下单、重启续接(K 线通道接线已在 023 前序完成)

### SOL 1 年 × 六种 K 线(2026-09-18 追加实测)

- 结果文档: `backtest-2026-09-18-sol-6intervals.md`
- 六组全过: 1m 515 笔 / 5m 548 / 15m 583 / 1h 683 / 4h 181 / 1d 26, 拒单全 0
- 交易收入(虚拟 10 万/年): +5,850 ~ +6,085(细粒度) / +5,554(4h) / +4,404(1d)
  → 折算真实 1 万 **+440 ~ +609 U/年**;份额 +20.4% ~ +73.5%
- **净贡献随粒度转正**: 1m/5m/15m/1h −0.2~−0.6pp;4h **+1.67pp**;1d **+4.85pp**
- 交叉通道六组均只触发 1 次(= R1 建仓), 真卖 0 → 与 QQQB 一致
- 修引擎真 bug 2 个(六周期暴露): ①`resample_complete` 缺口守卫硬编码输入粒度=1 分钟 →
  5m/15m/1h/4h/1d 重采样 0 桶 → ATR 永不就绪 → 全程 0 笔(新增回归测试
  `resample_accepts_coarser_input`);②4h/1d 主序列上预热 24h 只有 1~6 根 → 改为
  `max(24h, (atr_period+1)×atr_interval)`
- 待办: CLI 加"atr_interval 不得细于主序列"的显式校验(现在靠 0 桶 + 日志暴露)
