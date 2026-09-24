# ricow Lua 策略编写规范

> 本文档是 ricow 自定义策略的**唯一 API 规范**，供策略作者（含 AI 客户端）编写策略时阅读。
> 策略部署路径（两条，等价且都经编译门禁）：
> ① **手写**：写 `strategies/<name>.toml` + Lua 脚本 → 编译门禁 → 沙箱回测（`ricow backtest`）→ `ricow run`（默认 Dry Run）。
> ② **提交（推荐给 AI 客户端）**：`ricow create --name <n> --pair <p> [--script <file|->] [--param k=v]`（编译门禁 → 真实 K 线沙箱回测 → 报告 + `preview_id`，**不落盘**）
> → `ricow approve <preview_id>`（人工批准，得一次性 token）→ `ricow deploy <preview_id> --token <t>`（落盘 `<name>.toml` + `<name>.lua`）。
> `--script -`（或省略）从 stdin 读，**可直接把 AI 响应全文喂进去**（`extract_code` 会剥 ``` 围栏）。
> 三步不可跳过：create 不落盘、token 只能由人工 approve 生成且一次性、同名策略拒绝覆盖。
> 首次切实盘还需 Dry Run 累计满 `params.min_dry_run_hours`（默认 24 小时，设 0 关闭）。
> 对应实现: `ricow_strategy::lua`(沙箱引擎 + ctx API 注册)+ `ricow_strategy::exec`(exec 执行组件库)+ `ricow_strategy::indicators_api`(ta 指标库)。

## 一、策略结构

策略由 5 个回调函数组成，ricow 引擎按生命周期调用：

| 回调 | 时机 | 返回值 |
|:-----|:-----|:-----|
| `function on_init(ctx)` | 策略启动时调用一次 | 无 |
| `function on_tick(ctx)` | 每个行情更新时调用 | 订单数组（可为空 `{}`） |
| `function on_fill(ctx, fill)` | 订单成交时调用 | 无 |
| `function on_order_update(ctx, upd)` | 订单终态时调用 (拒单/撤单/过期回传, 2026-09-24 接线) | 无 |
| `function on_stop(ctx)` | 策略停止时调用 (停机清理: 撤单/平仓) | 无 |

所有回调的 `ctx` 参数为只读行情/账户快照；策略只能通过 on_tick 返回订单数组影响行为。

### on_fill 的 fill 字段（成交回调参数）

`on_fill(ctx, fill)` 的第二个参数是**成交事件表**，字段名固定如下（与引擎 `fill_to_table` 同源）：

| 字段 | 类型 | 含义 |
|:-----|:-----|:-----|
| `pair` | string | 交易对，如 `"ETHUSDT"` |
| `side` | string | `"buy"` / `"sell"` |
| `fill_price` | number | 成交价 |
| `fill_size` | number | 成交数量（基础币） |
| `fee` | number | 手续费（计价币，已扣） |

```lua
function on_fill(ctx, fill)
  -- 字段名是 fill_price / fill_size（不是 price / size）
  ctx:log(string.format("[fill] %s %s %.6f @ %.2f fee=%.6f",
    fill.pair, fill.side, fill.fill_size, fill.fill_price, fill.fee))
end
```

> **常见错误**：`fill.size` / `fill.price` **不存在**，读到的是 `nil` —— 直接进 `string.format("%.6f", nil)` 会抛错；写成 `fill.size or 0` 则**静默打成 0**（实测 2026-09-14 有生成策略因此把成交量/价打印成 `0.000000` / `0.00`，而订单行里的真实价是对的）。

### on_order_update 的 upd 字段（订单终态回调参数, 2026-09-24 接线）

`on_order_update(ctx, upd)` 的第二个参数是**订单终态事件表**（拒单/撤单/过期回传，与引擎 `update_to_table` 同源）。引擎在 `place_order` 返回 `Rejected` / `Cancelled` / `Expired` 时回调，让策略感知"订单没成"，避免挂单被拒后停摆：

| 字段 | 类型 | 含义 |
|:-----|:-----|:-----|
| `pair` | string | 交易对 |
| `status` | string | `"rejected"` / `"cancelled"` / `"expired"` |
| `filled_size` | number | 已成交数量（拒单恒 0） |
| `remaining_size` | number | 未成交数量 |
| `client_order_id` | string | 客户端订单号（引擎生成） |
| `exchange_order_id` | string | 交易所订单号（拒单时为空串） |

```lua
function on_order_update(ctx, upd)
  if upd.status == "rejected" or upd.status == "cancelled" then
    need_rehang = true  -- 挂单没成 → 下一 tick 重挂, 防停摆
  end
end
```

### on_stop 的停机清理语义 (008)

- 触发时机: `ricow stop` / `daemon` 停机指令 / 前台 `run` 收到 stdin `stop` / 管道 EOF(管理器消失) / Ctrl-C / 行情流中断 —— 任何退出路径都会调用一次
- **可选**: 未定义 `on_stop` 的脚本视为"未实现清理", 引擎不代为臆测; `ricow stop` 与 `run` 退出时会如实提示"该策略未实现清理, 如仍有挂单/持仓需手工处理"
- 判定方式: 脚本里存在名为 `on_stop` 的函数即为"已实现清理"(引擎据此决定提示文案)
- 清理里的下单与主循环一致: `ctx:place_order` 产生的成交同样计入统计并落 `fills` 表
- **Dry Run**(虚拟撮合): 交易所侧不存在本策略挂单/持仓, 引擎不做撤单兜底, 只调用脚本 `on_stop`
- **实盘**(011)停机顺序固定为: **停消费行情 → 策略 `on_stop` → 引擎撤单兜底 → (可选)平仓 → 残留复查 → 如实输出**; 两者**并存** —— `on_stop` 适合策略自有语义(记状态/撤销策略内部账), 引擎兜底保证"即使脚本没写清理也不留残单"
  - 撤单兜底只撤**本实例归属**的单(`clientOrderId` 带 `<策略名>-` 前缀); 非归属挂单只上报不撤(避免误撤手工单或其他实例的单)
  - 平仓仅在 `stop --close-all`(或启动 `--close-all`)时执行: 市价反向平掉该 pair 持仓, 数量按 `step_size` 向下取整, 不足 `min_qty` 则不平仓并如实说明; 平仓成交经用户流回写(清理后 ≤5s 吸干窗口)后落库
  - 重复停机只清理一次(幂等); 单个撤单失败不中断其余, 失败逐笔如实累计输出
- 策略若要区分"回测 / Dry Run / 实盘"语境: **用现有通道即可** —— `ctx:config_bool("<key>")` 由 TOML `params` 注入(如 `live = true`); 引擎**不**注入任何额外魔法字段(011 D10)

## 二、ctx API 清单（冒号调用）

### 行情

| 函数 | 返回 | 说明 |
|:-----|:-----|:-----|
| `ctx:price(pair)` | number? | 当前价（回测中为当前 bar 的 open，不含未来信息） |
| `ctx:now()` | table? | 当前 tick 时间 (UTC)：`{hour=, minute=, weekday=(1=周一), ymd=YYYYMMDD, ts=epoch秒}`。回测 = 本 tick 已开盘 bar 的 open_time (无前视)；通道不可用返回 `nil`（策略须 `if t then` 判断）。用于"每日固定时刻下单、其余 tick 只估值"的盘中策略。2026-09-11 新增 |
| `ctx:best_bid(pair)` | number? | 最优买价 |
| `ctx:best_ask(pair)` | number? | 最优卖价 |

### 持仓与余额

| 函数 | 返回 | 说明 |
|:-----|:-----|:-----|
| `ctx:position_side(pair)` | string | 净仓方向: `"long"` / `"short"` / `"none"` (合约 one-way 单侧; hedge 下为多空合并净值, 见下) |
| `ctx:position_size(pair)` | number | 净仓数量 (正数; hedge 合并后取净) |
| `ctx:position_entry(pair)` | number | 净仓开仓均价 (占优方向的加权均价) |
| `ctx:pos_size(pair, side)` | number | 指定方向仓数量 (side = `"long"` / `"short"`; 合约 hedge 双仓独立可见, 现货恒 long, 无仓返回 0) |
| `ctx:pos_entry(pair, side)` | number | 指定方向仓开仓均价 (无仓返回 0) |
| `ctx:balance(asset)` | number | 资产余额 (如 `"USDT"`; quote 跟随交易对报价币, 合约回测中为 quote 现金) |
| `ctx:net_pnl()` | number | 已实现净盈亏 (报价币计, 已扣手续费; 回测/Dry Run/实盘同一口径) — 2026-09-15 新增, 供策略自管回撤/止损 |
| `ctx:equity()` | number | 总权益: 现货 = 报价现金 + 持仓市值; 合约 = 钱包现金 + 未实现盈亏 — 2026-09-15 新增 |

> **盈亏政策属于策略** (2026-09-15, 020-platform-scope-trim): 平台**不再**提供亏损熔断/峰值回撤这类默认判断
> (原"两级亏损熔断"已删除)。策略用 `ctx:net_pnl()` / `ctx:equity()` 自己实现回撤与止损 ——
> 内置 `shannon_rebalance` 的 `dd_stop_pct` 参数即一条参考写法 (默认 0 = 关闭)。
>
> 持仓语义 (specs/backtest.md §五.6/§八 D9): 默认 `one-way` 模式同一交易对只有一个净仓, `position_*` 即全部信息;
> `hedge` 模式下多空可并存, 净仓查询 (`position_*`) 合并多空后取净 (net = 0 时 side 为 `"none"`),
> 精确的方向仓用 `pos_size` / `pos_entry` 查询。回测引擎 (BacktestContext) 支持方向仓; 实盘方向仓查询后续接入, 暂返回 0。
> Dry Run 虚拟净仓 (DryRunContext) 遵循同一不变式: `position_size > 0` 时 `position_side` = 建仓方向, 归零即报 `"none"`;
> 平仓归零后按原持仓方向再次开仓不会残留旧方向、数量不会累加 (2026-09-19 由 `specs/changes/024-dryrun-position-side/` 修复)。

### 配置参数（策略参数化）

| 函数 | 返回 | 说明 |
|:-----|:-----|:-----|
| `ctx:config_f64(key)` | number | 浮点参数 |

> **引擎级运行时参数(非策略 API, 写在 `[strategy.params]`)**: `initial_cash` —— Dry Run 的虚拟本金(缺省 100000;
> 设为与实盘相同的资金口径, Dry Run 的权益口径才与实盘可比, 见 `specs/changes/016-dryrun-initial-cash/`; 注意平台自 2026-09-16 起不再有 `[risk]` 配置面, 风控由策略自管 —— 见 `specs/changes/019-ai-assistant/spec.md` §七 R5);
> `min_dry_run_hours` —— 实盘前的 Dry Run 时长门禁(缺省 24, 设 0 关闭);
> `liq_warn_pct` —— 合约距强平告警阈值(缺省 15);
> `warmup_bars` —— 回测预热段长度(根数, 缺省 0): 声明 `atr_interval` / `regime_interval` 时 CLI 自动填
> (取 ATR 预热与趋势判据预热**的大者**; 后者 = 3×(regime_ema_period+1) 根高周期 bar),
> 该段只喂高周期指标(ATR / 趋势判据 EMA), **不进 tick 循环与报告**(2026-09-17 新增, 023);
> `notify_webhook` / `notify_chat_id` / `notify_events` / `notify_min_interval_secs` —— 出站通知(缺省关闭)。
| `ctx:config_i64(key)` | integer | 整数参数 |
| `ctx:config_str(key)` | string | 字符串参数 |
| `ctx:config_bool(key)` | boolean | 布尔参数 |

参数在提交策略时通过 `create_strategy` 的 `params` 传入（部署后存于策略 TOML）。
注意: 使用了 `config_xxx(key)` 的参数必须通过 `params` 传入, 未传的参数返回 0/空字符串, 不报错。

### 指标 API（基于已收盘 K 线，无前视）

指标输入为**已收盘**历史序列（当前未收盘 bar 不可见），数据不足返回 `nil`（用 `if v then` 判断）。

> **指标输入长度契约（2026-09-17 固定）**: 单标的路径下 `ctx:klines` 与全部指标的输入 = 同一条
> **尾窗序列，上限 100 根**（引擎按此裁剪，避免每 tick 克隆全量历史 —— 1m×79 天曾是 10^9 级拷贝）。
> 需要更长窗口的指标（如 EMA200）不属于单标的路径能力，请用组合信号模式或高周期序列。

| 函数 | 返回 | 说明 |
|:-----|:-----|:-----|
| `ctx:ema(pair, n)` | number? | 指数移动平均末值 |
| `ctx:sma(pair, n)` | number? | 简单移动平均末值 |
| `ctx:wma(pair, n)` | number? | 加权移动平均末值 |
| `ctx:rsi(pair, n)` | number? | RSI（Wilder 平滑） |
| `ctx:macd(pair)` | table? | `{main=, signal=, hist=}`（12/26/9） |
| `ctx:boll(pair, n, dev)` | table? | `{upper=, mid=, lower=}` |
| `ctx:atr(pair, n)` | number? | 平均真实波幅 |
| `ctx:adx(pair, n)` | number? | 平均趋向指数 |
| `ctx:stoch(pair, n)` | number? | 随机指标 %K |
| `ctx:cci(pair, n)` | number? | 顺势指标 |
| `ctx:roc(pair, n)` | number? | 变动率 |
| `ctx:mom(pair, n)` | number? | 动量 |
| `ctx:need_klines(role, tf, min_bars)` | — | **声明数据需求**（2026-09-24 新增，030 策略/引擎分层收敛）：在 `on_init` 里声明策略要哪些周期、多少根。`role` = `"primary"`（主时钟，至多一个，驱动逐 bar 推进）或 `"aux"`（辅助周期，供指标）。引擎按声明拉取/重采样供给，不读策略参数名、不猜根数。声明阶段只依赖 config（幂等，可重复调用）。 |
| `ctx:atr_tf(pair, tf, period)` | number? | **高周期 ATR**：显式传周期 `tf` 与 `period`（旧签名 `atr_tf(pair)` 已删）。序列须先在 `on_init` 用 `need_klines("aux", tf, ...)` 声明预装；引擎按桶缓存、无前视，可见 bar 不足 `period+1` 根 → `nil`。 |
| `ctx:close_tf(pair, tf)` | number? | **上一根已收盘高周期 bar 的收盘价**：显式传 `tf`（旧签名 `close_tf(pair)` 已删）。序列须 `need_klines` 声明预装；未预装/未收盘不可见 → `nil`。 |
| `ctx:ema_tf(pair, tf, period)` | number? | **高周期 EMA**：显式传 `tf` 与 `period`（旧签名 `ema_tf(pair)` 已删）。序列须 `need_klines` 声明预装；`ta` 的 EMA 用**首值种**，序列起点越早越准 → 预热长度见下方"装配责任"。 |
| `ctx:atr(pair, n)` | number? | 平均真实波幅（**主序列**口径；高周期见 `ctx:atr_tf`） |
| `ctx:ema_cross(pair, tf, fast, slow)` | table? | **信号序列 EMA 快慢线**：显式传 `tf` 与 `fast`/`slow`（旧签名 `ema_cross(pair)` 已删），返回 `{fast=…, slow=…}`。序列须 `need_klines` 声明预装；未预装/可见 bar 不足 → `nil`。 |
| `ctx:state_get(key)` | string? | **读策略持久化状态**（2026-09-22 新增，030 断点续接）：引擎启动时把上次会话保存的键值注回；键与值都是字符串，无记录 → `nil`。 |
| `ctx:state_set(key, value)` | — | **写策略持久化状态**（2026-09-22 新增，030 断点续接）：引擎在**每笔成交后与停机时**取快照落库（表 `strategy_state`），下次启动自动注回 —— 支撑“关机/中止不清仓、重启继续跑”。 |

数据不足阈值: EMA/SMA/WMA/BOLL/Stoch/CCI 需 ≥n 根; RSI/ATR/ROC 需 ≥n+1 根; MACD 需 ≥35 根; ADX 需 ≥2n 根。

> **高周期通道的装配责任（030，2026-09-24 收敛为声明驱动）**: 引擎不替策略猜周期，装配层负责把序列重采样后预装（缓存键 = `pair|tf`，同一 pair 可同时装多套）。
> - **声明驱动**: 策略在 `on_init` 用 `need_klines` 声明要哪些 `tf`、各要多少根，引擎按声明拉取/重采样 —— 回测与实盘/模拟盘**同一条装配路径**（不再有"实盘未接线"的差别）。
> - 回测两阶段：先跑一次 `on_init` 收集声明 → 按声明算预热根数并拉取全段 → 正式逐 bar 推进（预热段只喂高周期指标、**不进 tick 循环与报告**）。
> - 预热根数 = 各 `aux` 声明的 `min_bars × tf_ms / 主时钟_ms`（向上取整）取最大；趋势判据建议声明 `3×(regime_ema_period+1)` 根（`ta` 的 EMA 用首值种，种子残差 `(1−2/(n+1))^k`：n=200 时 201 根 13.5% / 402 根 1.8% / 603 根 0.25%，±3% 带下必须取 3×）。
> - **min_bars 语义（防陷阱）**: `min_bars` 是**该 tf 自身的最少根数**（不是主时钟根数，引擎按 `min_bars × tf_ms / 主时钟_ms` 换算预热）。要表达"至少 N 小时"这类余量，必须**换算成 tf 根数**再声明：`ceil(N小时 / tf)` 根 —— 如"24 小时"在 1h 下是 24 根、15m 下是 96 根、4h 下是 6 根。写死一个数字（如 24）会在 15m 下只留 6 小时，余量不足。
> - 重采样口径：只保留**完整桶**（首尾半桶与缺口桶丢弃，防"半小时当一小时"算错 ATR）。
> - 与 `ctx:atr` 的区别：`ctx:atr` 算的是**主序列**，`ctx:atr_tf` 才是高周期 ATR；两者口径不同，不可混用。

### 其他

| 函数 | 返回 | 说明 |
|:-----|:-----|:-----|
| `ctx:log(msg)` | 无 | 写日志 |
| `ctx:klines(pair)` | table? | 已收盘 K 线数组(无前视),每根 `{ts=, open=, close=, volume=}` (`ts` = bar open_time 的 epoch 秒, 2026-09-11 新增, 供盘中策略定位"当日会话起点")。单标的路径最多最近 100 根; 组合信号模式 (config 有 `universe` 键) 返回该 pair 美股信号线截至当前执行日的已收盘段, **尾窗封顶 400 根** (≥253 根可打分, 见 §四), 不套 100 cap |

### exec 执行组件（引擎内置，全局表，点号调用）

引擎内置 Rust 实现，加载策略时注册为全局表 `exec`，所有 Lua 策略（含用户策略）可直接调用；
脚本内可重新定义同名函数覆盖默认实现（复制即自定义）。状态（计数/标记）由策略脚本持有，库内只有纯函数/取价。

| 函数 | 返回 | 说明 |
|:-----|:-----|:-----|
| `exec.levels(lower, upper, n, geo)` | table | 档位生成（ladder）: lower~upper 间 n 档，geo=true 等比（对数刻度）否则等差；n<2 返回空 |
| `exec.pullback_triggered(side, px, extreme, pct, abs)` | boolean | 回调触发判断: buy 跌 pct/abs 触发，sell 对称；pct/abs 传 nil 不启用 |
| `exec.detect_quote(pair)` | string | 报价资产推导（BNBUSDT → "USDT"），兜底 "USDT" |
| `exec.ticks_per(interval_secs, bar_secs)` | integer | 间隔换算为 tick 数，至少 1；interval/bar_secs ≤ 0 返回 1（CLI 直跑缺省 bar_seconds=0 的守卫） |
| `exec.slice_due(tick_count, start_tick, slices_sent, ticks_per_slice)` | boolean | TWAP 切片到期: 首片立即（slices_sent==0），之后每 ticks_per_slice 一片 |
| `exec.side_order(ctx, pair, side, size)` | table | 对手价限价单（买 best_ask / 卖 best_bid），无盘口回退市价 |

示例（内置 dca.lua 的间隔换算）:

```lua
ticks_per_interval = exec.ticks_per(ctx:config_i64("interval_secs"), ctx:config_i64("bar_seconds"))
```

## 三、订单格式（on_tick 返回数组）

每个订单是一个表：

```lua
{
    pair = "BNBUSDT",          -- 交易对
    side = "buy",              -- "buy" 或 "sell"
    size = 0.01,               -- 数量
    price = 600.0,             -- 可选，省略为市价单
    order_type = "limit",      -- "limit" 或 "market"
    reduce_only = false,       -- 可选，默认 false
    position_side = "long",    -- 可选 (合约 hedge): "long"/"short" 指定作用方向仓; 缺省按 one-way 净仓语义
}
```

- `position_side` 仅在合约 hedge 模式有意义 (specs/backtest.md §五.6): `buy + "long"` 加多、`sell + "long"` 平多;
- **实盘(012)**: 合约实盘下单须与账户持仓模式一致 —— 引擎按策略 `position_mode` 装配(声明 `hedge` 时启动自动切双向并幂等容忍 "No need to change position side."); 停机兜底平仓由引擎生成: one-way 用 `reduceOnly`, hedge 用 `positionSide` 且**不带** `reduceOnly`(fapi 两者同带会被拒)。
  `buy + "short"` 平空、`sell + "short"` 加空。one-way 模式或现货下可省略 (省略 = one-way 语义)。
- 现货: `sell` 恒为平多, 无币可卖时拒单 (禁做空, §四.4)。

## 四、组合信号模式与撮合参数回写（2026-09-09 bs_momentum Lua 化）

> **注 (2026-09-11)**: 该机制的唯一内置消费者 `bs_momentum.lua` 已删除 (真实成交轨期望 ≈0)。
> 本节描述的**引擎能力保留** (通用), 但当前**无内置策略使用**; 新策略可照此接入。

### 组合信号模式（ctx:klines 长窗; 原 bs_momentum 轮动用）

- 触发：config.params 含 `universe` 键（逗号串 = 池内成交轨 symbol，装配层注入）。
- 数据双轨：**信号轨** = 美股 Nasdaq 日线（key = 成交轨 pair 名），引擎在
  `run_portfolio_backtest` 以 `signal_klines` 装载后，`ctx:klines(pair)` 返回该 pair
  信号线**截至当前执行日**的已收盘段（截断条件 = bar.open_time < 组合全局 tick 时间，
  即当 tick 各 pair bar open_time 最大值；数据缺口日该 pair 无 bar 时截断仍按全局时间，
  不悬空）——脚本永不见未来 bar，无前视。信号线不足 253 根的标的由策略脚本自行跳过
  （宁缺毋滥）。`ctx:price(pair)`/`ctx:pos_size(pair, side)` 按 pair 路由（成交轨当前
  bar open / 实际持仓），组合内逐只可查。
- 长度边界：组合信号模式返回该 pair 信号线截至当前执行日已收盘段，**尾窗封顶最近
  `SIGNAL_TAIL`(400) 根**（007 v2，R1 十年回测性能前提；打分只需 ≥253 根，400 留足
  ROC252/EMA200(t-20)/52w 高余量；超长段只裁旧根不裁未来，无前视不变）。单标的路径
  （无 universe）维持最多最近 100 根不变（回归约束，行为边界）。
- 成交轨 = 币安 bStock 1d（spot = bstock base+USDT；futures = fapi EQUITY us+USDT），
  R1 研究回测可切 `--market us` = Nasdaq 日线近似（忽略 bStock 溢价/时差，报告注明）；
  撮合/成交按成交轨 bar open；信号日 T（美股收盘）→ 成交于其后首根新 bar，引擎截断
  语义天然实现（T 日信号线 open_time < T+1 tick 时间）。
- v2 策略参数（bs_momentum.lua 007）：`market_gate`（池内 SPY symbol；空 = 门不激活，
  006 兼容）、`rebalance_days`（周频闸，装配层注入 5）、`min_candidates`（持仓下限，
  装配层注入 3）——市场门/周频/下限全部指标基于美股信号线（数据源铁律），详见
  bs_momentum.lua 头注释。

### 撮合参数回写（费率/滑点/杠杆与撮合层同源）

`BacktestContext::new` 在 resolve 出最终 `BacktestParams`（内置默认 < 策略 TOML
`[backtest]` < CLI 覆盖）后，把 `fee_maker_bps/fee_taker_bps/slippage_bps/leverage/
mmr_pct/funding_rate_8h` 统一回写进 ctx.config.params。策略经 `ctx:config_f64` 读到的
值与撮合层 FeeModel/账户模型完全一致——未显式配置的默认值（如 taker 10bps）也可读。
Lua 脚本调仓预算按「净回笼口径」闭合的前提（策略按 `size = budget/(px×buy_f)` 反推，
实付恰 = 预算，撮合 Decimal 仲裁兜底）。

## 五、Lua 语法注意点（重要）

1. 数组/表索引从 **1** 开始（不是 0）。
2. 表即数组: `{...}` 既可作数组也可作 map；订单列表是数组，每个元素是 map。
3. `nil` 语义: 函数返回 nil 表示"无值"（如数据不足的指标），用 `if v then` 判断后再用。
4. 模块级 `local` 变量在回调间**不保留**；跨 tick 保留状态用全局变量（如 `tick_count = 0`）。
5. 回调不可阻塞: 不要调用 sleep/网络等（沙箱已禁 os/io 库，本来也调不了）。
6. 只使用本规范列出的 API，不要调用未注册函数（运行期报错并记日志）。
7. 浮点精度: 0.1 这类小数在 Lua number 中非精确表示，下单数量建议用整数或可接受的精度。

## 六、完整示例：BNB EMA 交叉策略

```lua
-- EMA 交叉: 快线上穿慢线开多, 下穿平多
position_open = false

function on_tick(ctx)
    local fast = ctx:ema("BNBUSDT", 3)
    local slow = ctx:ema("BNBUSDT", 10)
    -- 数据不足 (nil) 时跳过, 等足够历史
    if not fast or not slow then
        return {}
    end
    local orders = {}
    if fast > slow and not position_open then
        position_open = true
        orders[#orders + 1] = {
            pair = "BNBUSDT", side = "buy", size = 0.1, order_type = "market"
        }
    elseif fast < slow and position_open then
        position_open = false
        orders[#orders + 1] = {
            pair = "BNBUSDT", side = "sell", size = 0.1, order_type = "market"
        }
    end
    return orders
end
```

## 七、安全边界

- 沙箱只加载 base/table/string/math/utf8 库，**无 os/io/debug/package/coroutine**。
- `require` / `loadstring` / `loadfile` / `dofile` 不可用；无文件、网络、进程访问。
- `pcall` / `xpcall` 不可用（错误捕获被禁用, 保证指令预算错误必达引擎层, 无法被脚本吞掉后继续烧 CPU）。
- 指令预算: 单 tick 最多 1,000,000 条指令（每个回调周期独立重置），死循环会被中断并记日志。
- 内存上限: 单实例 64MB（防 string.rep 等单次调用绕过指令预算造成 OOM）。
- `print` 重定向到日志，不污染 stdout。

## 八、交易所与市场

- **Binance 现货**：默认费率 10bps（maker/taker 同）；交易对用现货格式（如 `BNBUSDT`、`ETHUSDT`）。
- **Binance USDT-M 合约**：策略 TOML 声明 `market = "futures"`（或 `ricow backtest --market futures`）；默认费率 maker 2bps / taker 5bps；
  可声明 `position_mode = "one-way" | "hedge"`（订单里的 `position_side` 只在此有意义）。合约规则见 specs/backtest.md §五。
- **bStock（美股代币）**：现货 `base+USDT`（如 `TSLABUSDT`），合约走 fapi EQUITY；报价币为 USDT。
- 回测/实盘下单路径共用同一套 ctx/exec API，策略脚本不区分市场（差异由 TOML 的 market/position_mode 与撮合层承担）。
- **Hyperliquid 已于 2026-09-12 全量移除**（该适配从未落地, 暂不考虑; 半成品代码不作为文档承诺）——当前支持 Binance 现货 / USDT-M 合约 / bStock。

## 九、策略文件放哪(用户自建策略)

策略文件统一放**项目根**的 `strategies/` 目录(运行时自动创建,已被 git 忽略,用户策略默认私有):

```
<项目根>/strategies/<name>.toml        # 策略配置 (含参数)
<项目根>/strategies/scripts/<name>.lua # Lua 脚本 (script_path 引用)
<项目根>/ricow.db                      # 运行时数据 (行情缓存/成交/盈亏快照)
```

新建步骤(**推荐路径** = `create` 闭环, 全程带编译门禁 + 真实 K 线沙箱回测):

1. `ricow create --name <name> --pair <pair> --script <你的.lua>`(省略 `--script` 或写 `-` = 从 stdin 读全文)
   —— 引擎先做编译门禁, 再用真实 K 线沙箱回测, 通过则生成**不落盘**的 preview;
2. `ricow approve`(必须在**你自己的交互终端**里逐字确认)拿到一次性 token;
3. `ricow deploy <preview_id> --token <token>` 落盘 `strategies/<name>.toml` + `strategies/scripts/<name>.lua`(同名拒绝覆盖);
4. 回测:`ricow backtest --strategy <name>`(TOML 已含 pair 时可省 `--pair`);
5. 启动:`ricow run <name>`(Dry Run;TOML 里 `enabled = false` 会被拒绝启动)。

**没有独立的"策略模板文件"**: 样板就是下节的 `strategies/builtin/shannon_rebalance.lua`, 直接读它照写。
手工建策略(不走 create 闭环)同样支持: 自己写 `strategies/<name>.toml` + `strategies/scripts/<name>.lua`,
TOML 的 `params` 里用 `script_path` 引用脚本(相对 `strategies/` 或绝对路径);旧部署(TOML 内嵌 `script` 代码字符串)依然兼容。

目录定位:默认取**当前工作目录**(在项目根运行 `ricow`);从其他目录运行可设
`RICOW_ROOT=<项目根>`;数据库路径可单独用 `RICOW_DB=<path>` 覆盖。

内置脚本(shannon_rebalance 策略样板 + executors/ 执行模式示例)均为 Lua 脚本,参考实现见
`strategies/builtin/`(git 跟踪,与用户策略同目录,复制即自定义):

- `strategies/builtin/shannon_rebalance.lua` — 香农 50:50 中轴再平衡(**唯一策略样板**)
- `strategies/builtin/executors/dca.lua` / `twap.lua` / `vwap.lua` — 定时定投 / 时间加权分批 / 成交量加权分批(间隔按 tick 计数,需 `bar_seconds` 参数,CLI 按 interval 自动注入; vwap 参考价 = 已收盘 K 线成交量加权均价, 无成交量回退市价)
- `strategies/builtin/executors/pullback.lua` — 新高后回撤买入
- `strategies/builtin/executors/ladder.lua` — 区间分档挂限价单

> 执行模式示例(executors/)**不是策略**: 它们 = 最简信号(定时/回调/一次性挂单)+ 调 exec.* 执行;
> 复制后改信号部分即成为你自己的策略。**builtin 脚本为编译期嵌入(include_str!), 直接改文件不重编译不生效**;
> 自定义请复制到 `strategies/scripts/` 再改。

直接 `ricow backtest --strategy shannon_rebalance --pair ETHUSDT` 即可运行(引擎自动注入内置脚本;
**交易对必须带报价币**, 现货用 `ETHUSDT` 而非 `ETH`, 否则交易所返回 `Invalid symbol`);
复制 `strategies/builtin/shannon_rebalance.lua` 或 `strategies/builtin/executors/*.lua` 到
`strategies/scripts/` 修改即自定义(`strategies/` 下除 `builtin/` 外均被 git 忽略,
用户策略默认私有;想入库自行调整 `.gitignore`)。

