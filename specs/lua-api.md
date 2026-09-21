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

策略由 4 个回调函数组成，ricow 引擎按生命周期调用：

| 回调 | 触发 | 返回值 |
|:-----|:-----|:-----|
| `function on_init(ctx)` | 策略启动时调用一次 | 无 |
| `function on_tick(ctx)` | 每个行情更新(盘口) | 订单数组（可为空 `{}`） |
| `function on_quote(ctx, pair)` | 盘口更新, 带来源 pair(多标的用; 与 on_tick 语义相同) | 订单数组 |
| `function on_bar(ctx, series, bar)` | 某条**声明的序列**新收盘一根(028; 见 §十) | 订单数组 |
| `function on_timer(ctx, label)` | 策略**自定节奏**到点(028 `data:timer`; 回测虚拟钟 / 实盘墙钟) | 订单数组 |
| `function on_fill(ctx, fill)` | 订单成交时调用 | 无 |
| `function on_stop(ctx)` | 策略停止时调用 (停机清理: 撤单/平仓) | 无 |

写哪个就派发哪个(引擎按脚本里是否定义来选路径); 旧策略只写 `on_tick`/`on_fill`/`on_stop` 一样跑。
想自己定数据来源/标的/周期/节奏 → 见 **§十 声明式数据面**。

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
> 内置 `shannon_grid` 的 `dd_stop_pct` 参数即一条参考写法 (默认 0 = 关闭)。
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
> `notify_webhook` / `notify_chat_id` / `notify_events` / `notify_min_interval_secs` —— 出站通知(缺省关闭)。
| `ctx:config_i64(key)` | integer | 整数参数 |
| `ctx:config_str(key)` | string | 字符串参数 |
| `ctx:config_bool(key)` | boolean | 布尔参数 |

参数在提交策略时通过 `create_strategy` 的 `params` 传入（部署后存于策略 TOML）。
注意: 使用了 `config_xxx(key)` 的参数必须通过 `params` 传入, 未传的参数返回 0/空字符串, 不报错。

### 指标 API（基于已收盘 K 线，无前视）

指标输入为**已收盘**历史序列（当前未收盘 bar 不可见），数据不足返回 `nil`（用 `if v then` 判断）。

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

数据不足阈值: EMA/SMA/WMA/BOLL/Stoch/CCI 需 ≥n 根; RSI/ATR/ROC 需 ≥n+1 根; MACD 需 ≥35 根; ADX 需 ≥2n 根。

### 其他

| 函数 | 返回 | 说明 |
|:-----|:-----|:-----|
| `ctx:log(msg)` | 无 | 写日志 |
| `ctx:klines(pair)` | table? | 已收盘 K 线数组(无前视),每根 `{ts=, open=, high=, low=, close=, volume=}` (`ts` = bar open_time 的 epoch 秒), 行形状与句柄 `s:bars(n)` 同一个构造函数。**028 起全段返回**(原"单标的 100 根 cap"与"组合信号模式 400 根尾窗"随 `universe`/`signal_klines` 一并退役); 要控体量由策略自己 `data:series{...bars=N}` 声明窗口 |

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

### ~~组合信号模式（universe + signal_klines 预装）~~ —— 已于 028 退役

> **已删除(028 D7)**: `universe` 配置键、`signal_klines`/`SIGNAL_TAIL` 装载与 `set_signal_klines`
> 全部移除。原因: "引擎替策略预装信号线"属于平台替策略做决定, 与"完整逻辑在策略里"冲突。
> 迁移: 策略在脚本里**自己声明**要哪条序列(`data:series{...}`, 可用不同 source/symbol/interval,
> 例如 `source="nasdaq"` 的信号日线 + 币安成交轨同时声明), 引擎只负责按声明取数与按 `close_time` 派发 —— 见 §十。
> 快照覆盖的标的同样改为按声明推导(配置 pair ∪ 序列标的 ∪ `market:subscribe` 的 pair)。

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
- `require` / `loadstring` / `loadfile` / `dofile` 不可用；沙箱内**无文件/进程能力**，
  也没有原生 socket —— 网络只能走平台给的 `http:get(url)` 通道(仅 GET, 墙钟超时 10s, 响应体上限 5MB; 见 §十)。
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

**没有独立的"策略模板文件"**: 样板就是下节的 `strategies/builtin/shannon_grid.lua`, 直接读它照写。
手工建策略(不走 create 闭环)同样支持: 自己写 `strategies/<name>.toml` + `strategies/scripts/<name>.lua`,
TOML 的 `params` 里用 `script_path` 引用脚本(相对 `strategies/` 或绝对路径);旧部署(TOML 内嵌 `script` 代码字符串)依然兼容。

目录定位:默认取**当前工作目录**(在项目根运行 `ricow`);从其他目录运行可设
`RICOW_ROOT=<项目根>`;数据库路径可单独用 `RICOW_DB=<path>` 覆盖。

内置脚本(shannon_grid 策略样板 + executors/ 执行模式示例)均为 Lua 脚本,参考实现见
`strategies/builtin/`(git 跟踪,与用户策略同目录,复制即自定义):

- `strategies/builtin/shannon_grid.lua` — 香农 50:50 中轴再平衡(**唯一策略样板**)
- `strategies/builtin/executors/dca.lua` / `twap.lua` / `vwap.lua` — 定时定投 / 时间加权分批 / 成交量加权分批(间隔按 tick 计数,需 `bar_seconds` 参数,CLI 按 interval 自动注入; vwap 参考价 = 已收盘 K 线成交量加权均价, 无成交量回退市价)
- `strategies/builtin/executors/pullback.lua` — 新高后回撤买入
- `strategies/builtin/executors/ladder.lua` — 区间分档挂限价单

> 执行模式示例(executors/)**不是策略**: 它们 = 最简信号(定时/回调/一次性挂单)+ 调 exec.* 执行;
> 复制后改信号部分即成为你自己的策略。**builtin 脚本为编译期嵌入(include_str!), 直接改文件不重编译不生效**;
> 自定义请复制到 `strategies/scripts/` 再改。

直接 `ricow backtest --strategy shannon_grid --pair ETHUSDT` 即可运行(引擎自动注入内置脚本;
**交易对必须带报价币**, 现货用 `ETHUSDT` 而非 `ETH`, 否则交易所返回 `Invalid symbol`);
复制 `strategies/builtin/shannon_grid.lua` 或 `strategies/builtin/executors/*.lua` 到
`strategies/scripts/` 修改即自定义(`strategies/` 下除 `builtin/` 外均被 git 忽略,
用户策略默认私有;想入库自行调整 `.gitignore`)。

## 十、声明式数据面(028): 策略自己决定数据

**一句话**: 引擎不再写死喂哪条 K 线 —— 策略在脚本里声明"我要什么数据、什么周期、什么时候被叫醒",
平台只负责**取数(含预热)、按 `close_time` 无前视派发、以及交易/账户这一侧**。

### 10.1 声明(脚本顶层或 `on_init` 里调用)

```lua
local eth = data:series{
    id        = "eth1h",        -- 策略侧标识(回调 on_bar 里用它区分)
    source    = "binance_spot", -- 内置源: binance_spot / binance_futures / nasdaq / yahoo
    symbol    = "ETHUSDT",      -- 源原生写法(美股 = QQQ / SPY 等)
    interval  = "1h",           -- 1m/3m/5m/15m/30m/1h/2h/4h/6h/8h/12h/1d/3d/1w
      bars      = 300,            -- 策略可见尾窗根数(默认 300; **下限 = max(自报 min_bars, 2)**, 引擎不从指标
                              --   周期反推; **bars 或 min_bars 超过 5000 都硬报错**)
    min_bars  = 60,             -- 预热下限; 不足直接报"序列过短"(不会拿半截指标做决策)
    drive     = true,           -- true = 该序列收盘时回调 on_bar(回测里它同时是主时钟)
    price     = "close",        -- close(默认) / adjclose(仅 Yahoo 支持; 其它源用它会硬报错)
}
```

- `data:subscribe{...}` = **声明 + 驱动**(等价 `drive = true`, 语义更直白: "我要被它叫醒"),
  **不返回句柄**(FR-008: 收增量归 `subscribe`, 拿句柄归 `series`)。要"既有句柄又被驱动"请写
  `data:series{..., drive = true}`; 同一 `id` 先 `series` 再 `subscribe` 会报错(免得声明与句柄分裂)。
- `market:subscribe{ pair = "BTCUSDT" }` = 订阅盘口 → 该 pair 的行情更新回调 `on_quote(ctx, pair)`。
- `data:timer{ label = "t20", secs = 20 }` / `{ label = "open", at = "09:30", tz_offset_minutes = -240 }`
  = 自定节奏 → 回调 `on_timer(ctx, label)`(实盘/Dry Run 走**墙钟**; 回测按**虚拟钟刻度**, 同一份
  `TimerScheduler`)。`secs` 与 `at` **只能给一个**(同给会报错)。刻度比节奏粗时(如 `secs=30` 配
  1h 刻度)单刻度最多补 10 次, 之后按原相位重对齐(不连发)。
- 声明时机: 脚本顶层**或** `on_init` 都可以(引擎在 `on_init` 之后装配数据面)。
- **顶层会被执行两次**(回测/`create` 路径): 一次用于判定"该策略是否走声明路径"(`declared_series`),
  一次是真正装配; `on_init` 与各回调只跑一次。因此**顶层只做声明与幂等操作**(`data:*`/`market:*`/
  `data:timer`/读配置), 不要放有副作用的动作(下订单、写文件、`http:get` 取数) —— 那些放回调里。
- **单时间轴 + 逐标的撮合价(回测)**: 声明驱动的回测只有**一条**时间轴 =
  第一条**驱动**序列(`drive = true`); 每个刻度内各标的的成交参考价 = **该标的自己那条序列**
  此刻"正在形成" bar 的 `open`(不是主时钟的价)。
- **日线 `open_time` 是"日期对齐"值**(如 00:00Z / 00:00 本地), 而真实开盘在**交易时段开始**时;
  因此把日线序列当作**交易标的**、且 tick 时刻早于当日真实开盘时, 该 tick 的参考价会是"当天的 open"。
  回测请让**主时钟粒度 ≤ 交易标的粒度**(或直接用盘中序列), 否则成交价口径会偏乐观。
  (美股份额: 日线 `open_time` = 00:00Z, 而盘前/盘中最早成交在 13:30Z 附近。)
- **跨周期一次补发多根 bar**: 同一刻度内补发的多根 bar, 其下单都用"该标的此刻正在形成的 bar 的 open"
  作参考价(**不做逐根参考价**) —— 这会低估换手/滑点, 但**不构成前视**(该价在刻度时刻已可见)。
- **取数失败 = 直接报错, 不降级(2026-09-21 明确定口径)**: 声明期/运行期回源失败(网络不可达 / 403 区域
  拦截 / 限流 / 超时)一律**如实抛出**(带原因 + `ricow data pull` 提示), **不会**"悄悄改用本地库数据继续跑" ——
  降级会让错误很难发现, 是明确不允许的。回测(`allow_fetch=false`)只读本地库, 缺数据同样是硬报错。
  > 唯一的例外是**运行期增量取数**: 失败只告警 + 把该序列置 `stale`(策略 `s:stale()` 可见, FR-016)且不退出主循环 ——
  > 这是 spec 明写的口径(实盘不因一次取数失败退出), 仍属"显式可见", 不是静默降级。
- **受限网络下取数**: 平台不提供代理配置项(网络环境不是产品能力); 走标准环境变量即可 ——
  `HTTPS_PROXY=http://127.0.0.1:1080 ricow data pull …`(reqwest 默认读系统代理; 本机实测 socks5 端口
  常同时支持 HTTP CONNECT, 故无需 `socks` 特性)。给策略的 `http:get` 不读系统代理(沙箱侧统一走宿主策略)。
- **同策略序列数上限 32 条**: 第 33 条声明硬报错(同名重复声明 = 替换, 不算新增; 校验在取数**之前**)。
- **撮合取价只认声明序列(硬规则)**: 声明驱动的回测里, **未声明序列的标的没有参考价** ——
  它的市价单会被**拒单**(计数进报告), 不会悄悄用别的标的的价成交; 装配时会打一行提示告诉你
  交易标的没在声明里。要交易一个标的, 就给它一条 `data:series{...}`(或 `data:subscribe{...}`);
  `drive = false` 的序列只提供句柄读数、**不提供撮合参考价**(它不参与时间轴), 要拿它当交易标的
  请用 `drive = true`(即使不写 `on_bar` 回调, 句柄也会随派发更新)。同一份 Lua 在 Dry Run/实盘
  同样按声明驱动, 但时钟是墙钟、增量取数是行情长出来的。
- 数据不够会**硬报错**并给出该敲的命令, 例如:
  `错误: 序列 yahoo:SPY@1d 在本地库没有可用数据(区间 …); 先拉取: ricow data pull --source yahoo --symbol SPY --interval 1d`

### 10.2 三个取数动词

| 调用 | 作用 |
|:-----|:-----|
| `data:series{...}` | 声明一条序列(装载 + 可选驱动), 返回**句柄** |
| `data:subscribe{...}` | 声明一条序列并**驱动**它(`drive = true`), 不返回句柄(读值用 `data:series`) |
| `data:history{ source=, symbol=, interval=, limit= }` | 临时取一段历史(不声明句柄); 无数据报错带 `data pull` 提示 |
| `http:get(url)` | 取任意 URL(仅 GET)。返回 `body, err` 两个值: 失败时 `body == nil` 且 `err` 是字符串(前缀可判别: `invalid_url` / `timeout` / `too_large` / `status` / `network`) |

### 10.3 句柄(声明返回的表)

| 成员 | 说明 |
|:-----|:-----|
| `s.id / s.source / s.symbol / s.interval / s.price / s.window / s.drive` | 声明回读(与 `on_bar` 的 `series` 参数同源) |
| `s:len()` | 当前已装载根数 |
| `s:stale()` | 增量取数是否失败过(失败置位, 恢复清除; 引擎不替策略决定要不要收敛) |
| `s:last()` | 最后一根 `{ts, open, high, low, close, volume}` |
| `s:close(back)` | 倒数第 `back` 根的收盘(`s:close()` = 最新一根) |
| `s:ema/sma/wma/rsi/atr/adx/stoch/cci/roc/mom(n)` | 指标(与 `ctx:ema` 等同源实现, 同一输入长度下逐位一致) |
| `s:macd()` → `{main,signal,hist}` / `s:boll(n)` → `{upper,mid,lower}` | 组合指标 |

句柄由**引擎派发**推进(同一根 bar 至多推一次, 不会重复计数); 策略不需要也不应该自己往里塞 bar。

### 10.4 三种驱动并存(策略自己选)

| 驱动 | 何时用 | 回调 |
|:-----|:-----|:-----|
| 盘口 | 需要实时买卖盘/做市/高频 | `on_quote(ctx, pair)`(或旧的 `on_tick`) |
| 序列收盘 | 按 1m/1h/1d 等**收盘**做决策(最常见) | `on_bar(ctx, series, bar)` |
| 定时器 | 与行情无关的节奏(每天 09:30、每 30 秒对账) | `on_timer(ctx, label)` |

三条路径的下单出口**完全相同**: 回调返回订单数组, 或调 `ctx:place_order{...}` —— 都在回调返回后
与其它订单同批落地, 不区分驱动类型。

### 10.5 回测/实盘的数据口径(重要)

- **回测只读本地库**(可复现): 先 `ricow data pull --source <源> --symbol <标的> --interval <周期> [--days N | --start YYYY-MM-DD]`, 再 `ricow backtest …`。回测期间**不联网**。
- **实盘/Dry Run** 允许回源补齐(缺口才拉), 增量取数失败会把该序列置 `stale`(策略可 `s:stale()` 感知)。
- 无前视的口径只有一条: **`close_time <= 当前时刻` 的 bar 才可见**。回测里"当前时刻" = 主时钟推进到的 bar 开盘时刻, 所以"看到第 k 根收盘"之后, 成交只能发生在第 k+1 根开盘。
- 主时钟 = 第一条 `drive = true` 的声明序列(回测据此推进账本)。
- 不支持原生周期但有可整除的细粒度数据时会**重采样**并在 `series.mode`(`native`/`resampled`)与 `series.feed_interval` 里如实标明。
