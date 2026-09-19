-- 香农 ETF 指数增加策略 (shannon_etf_accum) v3 (2026-09-18, 023 修订)
--
-- 目标(用户 2026-09-18 定稿): **增加交易收入**; 持续交易时**多仓仓位逐步增加**。
-- 不再是"跑赢满仓"。
--
-- 机制(虚拟账本 + 金叉建仓 + 锚点网格) —— 用户 2026-09-18 定稿:
--   · 虚拟账本(`v_cap = real_cash × leverage_mult`, 默认 1 万 × 10 = 10 万): 所有目标仓位与
--     下单量都按虚拟账本算。**不变量 I1**: 账本不是成交历史的累加器, 而是"10 万本金在参考价处
--     的 target 权重快照" —— 每次"卖不出去只记账"时按 `v_coin = target·v_cap/P_ref` 整本重算,
--     因此虚拟 coin 占比恒 ≤ target_ratio, 不会越滚越大。真实持仓(`ctx:position_size`)才是
--     "多仓逐步增加"的载体。
--   · R1 初次金叉(无平衡价): 锚 := `ask + spacing`; 虚拟账本在锚价建 target 权重; 用**当前市价**
--     算回平衡量 → 市价买入 → 成交后锚 = 成交价。**金叉/死叉只用于这一次建仓**。
--   · R4 网格(建仓后的**唯一**通道): 成交后 / 每 `rehang_secs` → 先撤后挂 `平衡价 ± spacing`
--     两张限价单(默认 `spacing = 2×ATR`, ATR 口径不变 = 高周期 × atr_period):
--       上面 `平衡价 + 2ATR` → **挂卖单**; 下面 `平衡价 − 2ATR` → **挂买单**;
--     量 = 虚拟账本精确回 target(即"保持虚拟仓位平衡"), 行情插针触及即成交,
--     成交后平衡价 := 成交价, 并撤余单按新平衡价重挂(任意时刻挂单 ≤ 2 张)。
--   · 建仓后**不再使用金叉/死叉** —— 交叉通道(R2/R3 市价进出)默认关闭(`enable_cross=false`),
--     仅作历史对照保留。R2/R3 原语义: 金叉且价 ≤ 锚−spacing → 市价买; 死叉且价 > 锚+spacing
--     → 市价卖(卖量受真实持仓兜底, 不够则零下单 + 整本重算账本)。
--   · R5 ATR: 高周期(默认 1h)× `atr_period`(14), 7×24 口径不变; 引擎按高周期桶缓存。
--   · R6 日线趋势判据(2026-09-18 用户定稿): 用**上一根已收盘**高周期(默认日线)bar 的 close
--     与其 EMA(`regime_ema_period`, 默认 200)比较, `regime_band_pct`(默认 0.03)= ±3% 带:
--       BULL : close > EMA×(1+band)   → 只买不卖(网格卖侧 + 建仓外的一切卖出都被拦)
--       BEAR : close < EMA×(1−band)   → 只卖不买(**含首次金叉建仓**, 用户 D2-A)
--       RANGE: 带内(含等号)           → 两侧正常(现行为)
--     被拦的一侧只是"不挂那一张单": **锚与虚拟账本都不动**(用户 D1-B), 并撤销该侧
--     上一轮挂单(否则牛市里旧卖单会留在盘上成交); 判据未就绪(通道未装/日线不足)→ 不下单。
--     判据序列由装配层预装(`regime_interval`, 默认 "1d"), 无前视; 日内判定不变。
--
-- 守卫(不满足就跳过并计数, 绝不硬下单):
--   · `spacing > min_spacing_pct × 价格` —— 保本线: 每往返毛收割 `(s/P)²/8`, 手续费 `0.1%×成交额`
--     ⇒ 净零点 = `s/P = 0.4%`(2026-09-18 修正; 旧值 0.2% 会让 0.2%~0.4% 的往返净亏);
--   · 单笔名义 ≥ `min_notional`(币安现货默认 5 USDT);
--   · 高周期 ATR 未就绪 → 不下单(不猜值);
--   · 买量受真实现金兜底(留手续费), 卖量受真实持仓兜底。
--
-- 用法:
--   ricow backtest --strategy shannon_etf_accum --pair QQQBUSDT --interval 1m --days 90 \
--     --cash 100000 --param atr_mult=3 --param min_spacing_pct=0.004
--   回测本金 = 虚拟本金(10 万); 折算到真实 1 万 = 数值 ÷ leverage_mult。
--   TOML: type = "shannon_etf_accum" + [strategy.params] pair/atr_interval/atr_period/atr_mult/
--         ema_fast/ema_slow/target_ratio/min_notional/rehang_secs/real_cash/leverage_mult/
--         min_spacing_pct/enable_cross(默认 false: 建仓后只用 ±spacing 网格, 不再用金叉死叉)
--         regime_filter(off|er|ema200, 内置缺省 ema200)/regime_interval(默认 "1d")/
--         regime_ema_period(默认 200)/regime_band_pct(默认 0.03)
--
-- 重启续接(实盘): 传 `resume_balance_price`(上次成交价) 即直接重挂; 有持仓但没给该参数
-- → 记"网格待机"并跳过, **不猜价**。

quote_asset = "USDT"
-- 虚拟账本
v_cap = 0
v_scale = 0
v_coin = 0
v_cash = 0
-- 状态
built = false
balance_price = nil
last_side = nil
last_bar_ts = nil
need_rehang = false
last_rehang_ts = nil
prev_fast = nil
prev_slow = nil
-- 计数
fill_count = 0
rehang_count = 0
cross_buy = 0
cross_sell_real = 0
cross_sell_virtual = 0
cross_golden_seen = 0
cross_death_seen = 0
cross_death_qualified = 0
regime_blocked = 0 -- 熊市禁买拦下的次数
cross_golden_qualified = 0 -- 信号卖出合格的次数
skip_trend = 0 -- 趋势过滤拦下的买入次数
-- 日线趋势判据(2026-09-18): BULL 不卖 / BEAR 不买 / RANGE 正常
regime_state = "?" -- 最近一次判定: bull|bear|range
regime_switch = 0 -- 状态切换次数
regime_block_buy = 0 -- 判据拦下的买侧(网格+建仓)次数
regime_block_sell = 0 -- 判据拦下的卖侧次数
regime_blocked_cancel = 0 -- 被拦侧撤销旧挂单的次数
regime_ready_skip = 0 -- 判据未就绪(通道未装/日线不足)→ 不下单的次数
ticks_bull = 0
ticks_range = 0
ticks_bear = 0
grid_fills = 0
skip_no_atr = 0
skip_thin = 0
skip_notional = 0
sell_short = 0 -- 需要卖但真实仓位不够 → 不挂单的次数
buy_capped = 0 -- 需要买但现金不够 → 被截断的次数
idle_logged = false

local function num(ctx, key, dflt)
    local v = ctx:config_f64(key)
    if v == nil or v == 0 then
        return dflt
    end
    return v
end

local function cfg_str(ctx, key, dflt)
    local v = ctx:config_str(key)
    if v == nil or v == "" then
        return dflt
    end
    return v
end

local function cfg_bool(ctx, key, dflt)
    local v = ctx:config_bool(key)
    if v == nil then
        return dflt
    end
    return v
end

-- 日线趋势判据(用户 2026-09-18 定稿, 逐字实现):
--   BULL : `close >  EMA200 × (1 + band)`  → 可以买, **不卖**
--   BEAR : `close <  EMA200 × (1 − band)`  → 可以卖, **不买**
--   RANGE: `EMA200 × (1 − band) ≤ close ≤ EMA200 × (1 + band)`(含等号) → 正常(两侧都放开)
-- 判据输入 = **上一根已收盘**高周期(默认日线)bar 的 close 与其 EMA —— 引擎侧按桶缓存,
-- 日内不变且无前视。通道未装 / 可见 bar 不足 period 根 → 返回 nil(未就绪, 由调用方决定不下单)。
local function regime_of(ctx)
    local pair = ctx:config_str("pair")
    local close = ctx:close_tf(pair)
    local ema = ctx:ema_tf(pair)
    if not close or not ema or ema <= 0 then
        return nil
    end
    local band = num(ctx, "regime_band_pct", 0.03)
    local upper = ema * (1 + band)
    local lower = ema * (1 - band)
    local state = "range"
    if close > upper then
        state = "bull"
    elseif close < lower then
        state = "bear"
    end
    if state ~= regime_state then
        regime_switch = regime_switch + 1
        regime_state = state
        ctx:log(string.format(
            "[shannon_etf_accum] 日线趋势判据 → %s (第 %d 次切换): close=%.4f ema=%.0f %.4f " ..
            "带 [%.4f, %.4f]%s",
            string.upper(state), regime_switch, close, num(ctx, "regime_ema_period", 200),
            ema, lower, upper,
            state == "bull" and " → 只买不卖" or (state == "bear" and " → 只卖不买" or " → 两侧正常")))
    end
    return state
end

local function target_ratio_of(ctx)
    return num(ctx, "target_ratio", 0.5)
end

-- 账本名义规模 = 倍数 x 基准。
--   leverage_basis = "cash"(默认, 现状): 基准 = 配置 real_cash(初始本金) -> 规模恒定,
--       账户复利增长会摊薄有效倍数(赚钱时降杠杆), 亏损时反而放大(见文档 §十八)。
--   leverage_basis = "equity": 基准 = 账户**当前权益** -> 倍数恒定, 赢时加仓、亏时自动缩仓。
local function leverage_basis(ctx)
    local b = cfg_str(ctx, "leverage_basis")
    if b == nil or b == "" then
        return "cash"
    end
    return b
end

local function virtual_cap(ctx)
    local mult = num(ctx, "leverage_mult", 10)
    if leverage_basis(ctx) == "equity" then
        local eq = ctx:equity() or 0
        if eq > 0 then
            return eq * mult
        end
    end
    return num(ctx, "real_cash", 10000) * mult
end

-- 恒定杠杆: 每 tick 把账本按"名义规模变化"等比缩放(保持币/现金权重, 等价于账本继续持有,
-- 只是规模跟着账户权益走)。cash 基准下 v_scale 恒定 -> 缩放系数恒为 1, 行为不变。
-- 绝对值(沙箱不含 math 库, 手写)
local function fabs(x)
    if x < 0 then
        return -x
    end
    return x
end

-- Kaufman 效率比 ER = |close_t - close_{t-n}| / 总路径 (主序列尾窗, 无前视)。
-- 实测(SOL 4h 2022-2025): 趋势期中位 0.27~0.32, 震荡期 0.14~0.16 —— 分离度优于 ADX/均线组合。
local function efficiency_ratio(ctx, pair, n)
    local ks = ctx:klines(pair)
    if not ks or #ks < n + 1 then
        return nil
    end
    local first = ks[#ks - n].close
    local last = ks[#ks].close
    local path = 0
    local prev = first
    for i = #ks - n + 1, #ks do
        local c = ks[i].close
        path = path + fabs(c - prev)
        prev = c
    end
    if path <= 0 then
        return nil
    end
    return fabs(last - first) / path
end

local function rebase_book(ctx, pair)
    local cap = virtual_cap(ctx)
    if cap <= 0 then
        return
    end
    local eq_book = v_coin * (ctx:price(pair) or 0) + v_cash
    if eq_book > 0 then
        -- 精确归一化: 让账本权益恰好 = 账户权益 x 倍数(有效倍数恒等于配置值)。
        -- 只按比例缩放, 保持币/现金权重 —— 等价于"账本按现有仓位继续", 不重置比例。
        local k = cap / eq_book
        if k > 1.0000001 or k < 0.9999999 then
            v_coin = v_coin * k
            v_cash = v_cash * k
        end
    end
    v_scale = cap
end

-- I1: 把虚拟账本整本重算成"10 万本金在参考价 P_ref 处的 target 权重快照"。
local function v_reset(ctx, p_ref)
    v_cap = virtual_cap(ctx)
    v_scale = v_cap
    local target = target_ratio_of(ctx)
    v_coin = target * v_cap / p_ref
    v_cash = (1 - target) * v_cap
    return v_coin
end

-- **虚拟账本在价格 P 处回到 target 权重所需的数量(份)** —— 这是唯一定量依据。
-- 只看虚拟账本(10 万)自己的持仓与现金, **与真实账户持有什么无关**(用户 2026-09-18 纠正:
-- 之前误按"目标 − 真实持仓"下单, 既让真实持仓参与算法, 又让"卖不够→只记账"永远触发不到)。
local function book_rebalance(ctx, side, p)
    local v_equity = v_coin * p + v_cash
    local want = target_ratio_of(ctx) * v_equity -- 目标持仓市值
    local cur = v_coin * p
    if side == "buy" then
        if want <= cur then
            return 0
        end
        return (want - cur) / p
    end
    if cur <= want then
        return 0
    end
    return (cur - want) / p
end

-- 下单量 = **虚拟账本回平衡量本身**(不做任何比例缩放)。
-- 用户口径(2026-09-18): "完全按虚拟仓位计算应该买入多少, 跟实际仓位没有任何关系" ——
-- 账户只有 1 万而账本是 10 万, 所以钱会用完; 资金不足只表现为**截断/跳过**(有计数),
-- 绝不用比例偷偷缩小下单量(那等于把用户要的放大效果取消掉)。
local function cap_size(ctx, side, size, p, fee_bps)
    local pair = ctx:config_str("pair")
    if side == "buy" then
        local cash = ctx:balance(quote_asset) or 0
        -- 余量 = 手续费 + 滑点(引擎市价单按 bar.open×(1±slippage) 成交) + f64 精度。
        -- 只留手续费不够: 顶满现金的市价单会被滑点推过余额线 → 引擎直接拒单(fill 恒 0)。
        local max_buy = cash / (p * (1 + (fee_bps + 20) / 10000))
        if size > max_buy then
            return max_buy
        end
        return size
    end
    -- Lua 只有 f64: 直接取"满仓"会被精度反咬 —— 例如持仓 0.7974481658692142982 经 f64 往返
    -- 变成 0.7974481658692144,**比真实持仓略大** → 引擎按"无仓可卖/超卖"处理, 卖单永远不成交
    -- (2026-09-18 实测: 网格卖单反复挂出但 0 成交)。故卖量留 1e-9 安全折扣。
    local pos = (ctx:position_size(pair) or 0) * (1 - 1e-9)
    if size > pos then
        return pos
    end
    return size
end

local function log_state(ctx, tag)
    local pair = ctx:config_str("pair")
    local price = ctx:price(pair) or 0
    local pos = ctx:position_size(pair) or 0
    local cash = ctx:balance(quote_asset) or 0
    ctx:log(string.format(
        "[shannon_etf_accum] %s 持仓 %.6f 权益 %.2f 现金 %.2f 锚 %s v_coin %.6f",
        tag, pos, cash + pos * price, cash,
        balance_price and string.format("%.4f", balance_price) or "nil", v_coin))
end

function on_init(ctx)
    local pair = ctx:config_str("pair")
    quote_asset = exec.detect_quote(pair)
    balance_price = nil
    need_rehang = false
    last_bar_ts = nil
    prev_fast = nil
    prev_slow = nil
    regime_state = "?"
    v_cap = virtual_cap(ctx)
    local resume = num(ctx, "resume_balance_price", 0)
    if resume and resume > 0 then
        balance_price = resume
        need_rehang = true
        v_reset(ctx, resume)
        ctx:log(string.format(
            "[shannon_etf_accum] 续接: 平衡价 = %.4f (来自 resume_balance_price)", resume))
    end
    ctx:log(string.format(
        "[shannon_etf_accum] init pair=%s quote=%s v_cap=%.0f(real=%.0f×%.1f) target=%.2f " ..
        "atr=%s×%d mult=%.2f ema=%d/%d min_notional=%.2f min_spacing=%.4f 建仓后交叉通道=%s",
        pair, quote_asset, v_cap, num(ctx, "real_cash", 10000), num(ctx, "leverage_mult", 10),
        target_ratio_of(ctx), cfg_str(ctx, "atr_interval", "1h"), num(ctx, "atr_period", 14),
        num(ctx, "atr_mult", 2), num(ctx, "ema_fast", 10), num(ctx, "ema_slow", 20),
        num(ctx, "min_notional", 5), num(ctx, "min_spacing_pct", 0.004),
        tostring(cfg_bool(ctx, "enable_cross", false))))
    if cfg_str(ctx, "regime_filter", "off") == "ema200" then
        ctx:log(string.format(
            "[shannon_etf_accum] 日线趋势判据: filter=ema200 interval=%s ema=%d band=%.3f " ..
            "(BULL 只买不卖 / BEAR 只卖不买 / RANGE 两侧正常; 判据未就绪不下单)",
            cfg_str(ctx, "regime_interval", "1d"), num(ctx, "regime_ema_period", 200),
            num(ctx, "regime_band_pct", 0.03)))
    end
end

function on_tick(ctx)
    local pair = ctx:config_str("pair")
    local target_ratio = target_ratio_of(ctx)
    local atr_mult = num(ctx, "atr_mult", 2)
    local ema_fast_n = num(ctx, "ema_fast", 10)
    local ema_slow_n = num(ctx, "ema_slow", 20)
    local min_notional = num(ctx, "min_notional", 5)
    local rehang_secs = num(ctx, "rehang_secs", 3600)
    local fee_bps = num(ctx, "fee_bps", 10)
    local min_spacing_pct = num(ctx, "min_spacing_pct", 0.004)
    -- 建仓后是否继续使用金叉/死叉通道(R2/R3, 市价进出)。默认 **false** ——
    -- 用户 2026-09-18 定稿: "买入后, 有了平衡价格, 就不再使用金叉和死叉", 改用 ±spacing 网格挂单。
    -- true 仅作历史对照(cross 通道 + 不挂网格)。
    local enable_cross = cfg_bool(ctx, "enable_cross", false)
    -- 日线趋势判据(2026-09-18): off(不判) | er(旧 ER 判据, 默认不启用) | ema200(三态, 用户定稿)。
    -- 注: Lua 默认 off; 内置策略的 CLI/TOML 缺省由装配层注入 "ema200"(见 commands/run.rs、backtest.rs)。
    local regime_filter = cfg_str(ctx, "regime_filter", "off")
    -- 入口对齐: true = 第一根 K 线即建仓(不等金叉), 供不同粒度/参数对照用
    local enter_at_start = cfg_bool(ctx, "enter_at_start", false)
    -- 信号通道选择(2026-09-18): cross(金叉死叉) | rsi | boll | none(不择时, 只跑网格)
    -- 说明: 本策略本质是均值回归(跌买涨卖); 趋势类的 MA 交叉在震荡里假信号多、且滞后,
    -- 因此提供 RSI / 布林带这类"摆动型"触发做对照, 并可用 SMA 趋势过滤禁止"接飞刀"。
    local signal_mode = cfg_str(ctx, "signal", "cross")
    local regime_filter = cfg_str(ctx, "regime_filter", "off")  -- off | er
    local er_period = num(ctx, "er_period", 20)
    local er_threshold = num(ctx, "er_threshold", 0.25)
    local regime_sma = num(ctx, "regime_sma", 50)
    -- 熊市(下跌趋势): ER >= 阈值 且 价 < SMA(n) → 禁止买入, 允许卖出(用户 2026-09-18 定稿)。
    -- 震荡与上涨照常; 数据不足时不拦(等同原行为)。
    local in_bear = false
    if regime_filter == "er" then
        local er_val = efficiency_ratio(ctx, pair, er_period)
        local sma_val = ctx:sma(pair, regime_sma)
        if er_val and sma_val and (ctx:price(pair) or 0) < sma_val then
            in_bear = er_val >= er_threshold
        end
    end

    local rsi_n = num(ctx, "rsi_n", 14)
    local rsi_buy = num(ctx, "rsi_buy", 30)
    local rsi_sell = num(ctx, "rsi_sell", 70)
    local boll_n = num(ctx, "boll_n", 20)
    local boll_dev = num(ctx, "boll_dev", 2.0)
    local trend_filter_sma = num(ctx, "trend_filter_sma", 0) -- >0: 价 < SMA(n) 时禁止买入

    -- ── 1. 决策节流: 只在 1m 收线时决策; 成交后的重挂不受节流限制(实时性要求)
    local k = ctx:klines(pair)
    local ts = nil
    if k and #k > 0 then
        ts = k[#k].ts
    end
    local must_rehang = need_rehang
    local new_bar = false
    if ts ~= nil then
        if ts ~= last_bar_ts then
            new_bar = true
        end
        if ts == last_bar_ts and not must_rehang then
            return {}
        end
        last_bar_ts = ts
    elseif not must_rehang then
        return {}
    end

    local price = ctx:price(pair)
    -- 停机日志需要: 末次价格 + 账户当前权益(用于算"有效倍数")
    last_price = price or last_price
    equity_val = ctx:equity() or equity_val
    if not price or price <= 0 then
        return {}
    end

    -- ── 2. 高周期 ATR(引擎按桶缓存, 每根高周期 bar 只算一次); 通道未就绪不下单
    local atr = ctx:atr_tf(pair)
    if not atr or atr <= 0 then
        skip_no_atr = skip_no_atr + 1
        if skip_no_atr == 1 or skip_no_atr % 240 == 0 then
            ctx:log(string.format(
                "[shannon_etf_accum] 高周期 ATR 未就绪(跳过 %d 次): interval=%s period=%d",
                skip_no_atr, cfg_str(ctx, "atr_interval", "1h"), num(ctx, "atr_period", 14)))
        end
        return {}
    end
    local spacing = atr_mult * atr

    -- ── 3. 保本线: spacing 必须 > min_spacing_pct×价格, 否则每往返净亏(净零点 = 0.4%)
    --    注意: 交叉通道用的是同一个 spacing, 所以这条守卫对两个通道一起生效。
    if spacing <= min_spacing_pct * price then
        skip_thin = skip_thin + 1
        if skip_thin == 1 or skip_thin % 240 == 0 then
            ctx:log(string.format(
                "[shannon_etf_accum] 间距低于保本线(跳过 %d 次): spacing=%.4f <= %.2f%%×价格=%.4f",
                skip_thin, spacing, min_spacing_pct * 100, min_spacing_pct * price))
        end
        last_rehang_ts = ts
        need_rehang = false
        return {}
    end

    -- ── 3.5 日线趋势判据(2026-09-18): BULL 拦卖 / BEAR 拦买 / RANGE 两侧正常。
    --    判据未就绪(通道未装 / 可见日线不足 period 根)→ **不下单**(与 ATR 未就绪同口径, 不猜值)。
    local block_buy, block_sell = false, false
    if regime_filter == "ema200" then
        local state = regime_of(ctx)
        if state == nil then
            regime_ready_skip = regime_ready_skip + 1
            if regime_ready_skip == 1 or regime_ready_skip % 240 == 0 then
                ctx:log(string.format(
                    "[shannon_etf_accum] 日线趋势判据未就绪(跳过 %d 次): interval=%s ema=%d → 不下单",
                    regime_ready_skip, cfg_str(ctx, "regime_interval", "1d"),
                    num(ctx, "regime_ema_period", 200)))
            end
            last_rehang_ts = ts
            need_rehang = false
            return {}
        end
        if state == "bull" then
            ticks_bull = ticks_bull + 1
            block_sell = true
        elseif state == "bear" then
            ticks_bear = ticks_bear + 1
            block_buy = true
        else
            ticks_range = ticks_range + 1
        end
    end

    -- ── 4. EMA 交叉(只在收线上判, 前值每次收线都要推进, 否则交叉会被漏掉)
    local fast = ctx:ema(pair, ema_fast_n)
    local slow = ctx:ema(pair, ema_slow_n)
    local golden, death = false, false
    if fast and slow and new_bar then
        if prev_fast ~= nil and prev_slow ~= nil then
            golden = prev_fast <= prev_slow and fast > slow
            death = prev_fast >= prev_slow and fast < slow
        end
        if golden then
            cross_golden_seen = cross_golden_seen + 1
        end
        if death then
            cross_death_seen = cross_death_seen + 1
        end
        prev_fast = fast
        prev_slow = slow
    end

    -- ── 5. 未建仓
    if not built then
        local pos = ctx:position_size(pair) or 0
        if pos > 0 then
            built = true
            if balance_price == nil then
                if not idle_logged then
                    idle_logged = true
                    ctx:log(string.format(
                        "[shannon_etf_accum] 检测到持仓 %.6f 但无平衡价 → 网格待机(不猜价); " ..
                        "重启请传 resume_balance_price", pos))
                end
                return {}
            end
            need_rehang = true
        else
            -- 入场条件(用户 2026-09-18 最新口径): **不要一开始就买入** ——
            --   交叉模式: 首次金叉(原 R1: 锚 = 卖价 + spacing, 账本在锚价建 50:50, 再算应买多少);
            --   RSI/布林模式: 首次出现买入信号; `enter_at_start=true` 仅用于口径对照(默认关)。
            local first_buy_sig = golden
            if signal_mode == "rsi" then
                local r0 = ctx:rsi(pair, rsi_n)
                first_buy_sig = r0 ~= nil and r0 <= rsi_buy
            elseif signal_mode == "boll" then
                local b0 = ctx:boll(pair, boll_n, boll_dev)
                first_buy_sig = b0 ~= nil and price <= b0.lower
            elseif signal_mode == "none" then
                first_buy_sig = false
            end
            -- 熊市判据(BEAR)拦建仓(用户 D2-A: "熊市可以卖, 不买" 含首次建仓)。
            if (first_buy_sig or enter_at_start) and block_buy and not in_bear then
                regime_block_buy = regime_block_buy + 1
                if regime_block_buy <= 3 or regime_block_buy % 24 == 0 then
                    ctx:log(string.format(
                        "[shannon_etf_accum] 建仓被熊市判据拦下(累计 %d 次): 日线 close 低于 " ..
                        "EMA×(1−%.3f) → 不买(等 RANGE/BULL 的金叉)", regime_block_buy,
                        num(ctx, "regime_band_pct", 0.03)))
                end
            end
            if (first_buy_sig or enter_at_start) and not block_buy and not in_bear then
                -- R1: 锚 = ask + spacing, 虚拟账本在锚价建 target 权重, 再按当前市价算买量
                -- 用户 2026-09-18 定稿(恢复原规则): 金叉且尚无平衡价格时,
                -- 锚 = 当前成交价 + 2*ATR(=spacing), 账本在锚价建 target 权重 50:50,
                -- 再按当前市价算"应买多少"(账本回平衡量)。
                balance_price = price + spacing
                v_reset(ctx, balance_price)
                local size = book_rebalance(ctx, "buy", price)
                v_scale = virtual_cap(ctx)
                size = cap_size(ctx, "buy", size, price, fee_bps)
                if size > 0 and size * price >= min_notional then
                    cross_buy = cross_buy + 1
                    ctx:log(string.format(
                        "[shannon_etf_accum] R1 初次建仓(%s): ema%d=%.4f/%.4f 锚=%.4f " ..
                        "(=ask+spacing) 虚拟账本 %.0f/%.0f → 市价买入 %.6f (≈%.2f %s)",
                        signal_mode, ema_fast_n, fast, ema_slow_n, balance_price, v_coin, v_cash,
                        size, size * price, quote_asset))
                    return {
                        { pair = pair, action = "cancel_pending" },
                        { pair = pair, side = "buy", size = size, order_type = "market" },
                    }
                end
                ctx:log(string.format(
                    "[shannon_etf_accum] R1 金叉但建仓名义 %.2f < min_notional %.2f, 放弃本轮",
                    size * price, min_notional))
            end
            return {}
        end
    end

    -- ── 6. 已建仓: 时间基准
    local now_ts = ts
    local t = ctx:now()
    if t and t.ts then
        now_ts = t.ts
    end
    if balance_price == nil then
        return {}
    end

    -- ── 7. 交叉通道(R2/R3): **默认关闭** —— 建仓后不再使用金叉/死叉。
    --   仅当 `enable_cross=true` 时作为历史对照启用(与网格互斥, 不挂 ±spacing 网格)。
    --   触发信号由 `signal` 决定: cross(金叉卖/死叉买) | rsi(超买卖/超卖买) | boll(上轨卖/下轨买)
    --   两类条件都必须满足: ① 信号 ② 价与锚的距离 > spacing(用户的 2ATR 约束)。市价成交。
    if enable_cross and new_bar then
        local buy_sig, sell_sig = false, false
        local sig_name = "金叉/死叉"
        if signal_mode == "rsi" then
            sig_name = "RSI"
            local r = ctx:rsi(pair, rsi_n)
            if r then
                buy_sig = r <= rsi_buy
                sell_sig = r >= rsi_sell
            end
        elseif signal_mode == "boll" then
            sig_name = "布林"
            local b = ctx:boll(pair, boll_n, boll_dev)
            if b then
                buy_sig = price <= b.lower
                sell_sig = price >= b.upper
            end
        elseif signal_mode == "none" then
            sig_name = "无信号"
            buy_sig, sell_sig = false, false
        else
            buy_sig = death
            sell_sig = golden
        end
        sell_sig = sell_sig and (price - balance_price) > spacing
        buy_sig = buy_sig and (balance_price - price) > spacing
        -- 趋势过滤(可选): 价 < SMA(n) 时禁止买入(不接飞刀); 卖出不受限。
        local trend_blocked = false
        if buy_sig and trend_filter_sma > 0 then
            local sma_v = ctx:sma(pair, trend_filter_sma)
            if sma_v and price < sma_v then
                buy_sig = false
                trend_blocked = true
            end
        end
        if trend_blocked then
            skip_trend = skip_trend + 1
        end
        -- 信号卖出
        if sell_sig then
            cross_golden_qualified = cross_golden_qualified + 1
            local want_sell = book_rebalance(ctx, "sell", price)
            local pos = ctx:position_size(pair) or 0
            if want_sell > 0 and pos >= want_sell and want_sell * price >= min_notional then
                cross_sell_real = cross_sell_real + 1
                ctx:log(string.format(
                    "[shannon_etf_accum] %s 卖出: 价 %.4f > 锚 %.4f + spacing %.4f → 市价卖出 %.6f (≈%.2f)",
                    sig_name, price, balance_price, spacing, want_sell, want_sell * price))
                return {
                    { pair = pair, action = "cancel_pending" },
                    { pair = pair, side = "sell", size = want_sell, order_type = "market" },
                }
            end
            -- 没有真实仓位或不够: 只记账 —— 按当时市价整本重算, 防虚拟仓位扩张
            cross_sell_virtual = cross_sell_virtual + 1
            local before = v_coin
            v_reset(ctx, price)
            balance_price = price
            ctx:log(string.format(
                "[shannon_etf_accum] %s 卖出(无仓可卖/不足): 价 %.4f > 锚+spacing, 真实持仓 %.6f < 应卖 %.6f " ..
                "→ 零下单, 虚拟账本按 10 万整本重算(累计 %d 次): v_coin %.6f → %.6f, 锚 := %.4f",
                sig_name, price, pos, want_sell, cross_sell_virtual, before, v_coin, balance_price))
            return {}
        end
        -- 信号买入(死叉 + 价低于锚−spacing); 熊市禁止买入
        if buy_sig and not in_bear then
            cross_death_qualified = cross_death_qualified + 1
            local size = book_rebalance(ctx, "buy", price)
            size = cap_size(ctx, "buy", size, price, fee_bps)
            if size > 0 and size * price >= min_notional then
                cross_buy = cross_buy + 1
                ctx:log(string.format(
                    "[shannon_etf_accum] %s 买入: 价 %.4f < 锚 %.4f − spacing %.4f → 市价买入 %.6f (≈%.2f)",
                    sig_name, price, balance_price, spacing, size, size * price))
                return {
                    { pair = pair, action = "cancel_pending" },
                    { pair = pair, side = "buy", size = size, order_type = "market" },
                }
            end
        end
    end

    -- ── 8. R4 网格(建仓后的唯一通道, 默认路径): `平衡价 ± spacing` 两张限价单 ——
    --   上面挂卖单、下面挂买单, 量 = 虚拟账本精确回 target(保持虚拟仓位平衡)。
    --   仅当 enable_cross=true(历史对照)时完全不挂网格。
    if enable_cross and signal_mode ~= "none" then
        return {}
    end
    -- 成交后 或 重挂周期到 → 先撤后挂锚 ± spacing
    local due = false
    if now_ts ~= nil and last_rehang_ts ~= nil then
        due = (now_ts - last_rehang_ts) >= rehang_secs
    end
    if not need_rehang and not due then
        return {}
    end
    -- 同一 tick 内只重挂一次: 引擎在 tick 内先撮合再回调, 回调里再重挂时 cancel_pending 撤不掉
    -- **已经成交**的那一批 → 会出现同价重复成交、账本被重复扣减、回平衡量衰减(2026-09-18 实测:
    -- 1h 组同价双买、纯网格只成 6 笔就枯死)。留到下个 tick 再挂。
    if need_rehang and now_ts ~= nil and last_rehang_ts == now_ts then
        return {}
    end

    local buy_px = balance_price - spacing
    local sell_px = balance_price + spacing
    -- 挂单价必须与市价可比(2026-09-18 实测真凶): 若锚长时间未更新(价格一路走开),
    -- 买单格会落在**市价上方**、卖单格落在市价下方 —— 引擎不会让这种"已被穿过的限价单"
    -- 立即成交, 于是网格永远不成交、锚冻结、枯死(1h 纯网格一年只成 26 笔)。
    -- 规则: 买单价 ≥ 市价 → 以市价成交(市场单); 卖单价 ≤ 市价 → 同理。
    local buy_type, sell_type = "limit", "limit"
    if buy_px >= price then
        buy_px = price
        buy_type = "market"
    end
    if sell_px <= price then
        sell_px = price
        sell_type = "market"
    end
    if buy_px <= 0 then
        last_rehang_ts = now_ts
        need_rehang = false
        return {}
    end
    -- 定量依据 = **虚拟账本自己的回平衡量**(只看 10 万账本, 与真实持仓无关), 再按比例落到真实账户。
    local want_buy = book_rebalance(ctx, "buy", buy_px)
    -- 熊市(ER + 价<SMA 判为下跌趋势): 网格买侧一并禁止(只允许卖出)。
    if in_bear and want_buy > 0 then
        regime_blocked = regime_blocked + 1
        want_buy = 0
    end
    local want_sell = book_rebalance(ctx, "sell", sell_px)
    -- 日线趋势判据(2026-09-18): BULL 不卖 / BEAR 不买 —— **只拦该侧挂单, 锚与账本都不动**
    -- (用户 D1-B)。位置关键: 必须在下面"卖量 > 真实持仓 → 整本重算 + 锚 := 市价"之前,
    -- 否则被拦的卖侧会走那条路径, 把锚移动掉, 与 D1-B 矛盾。
    if block_sell and want_sell > 0 then
        regime_block_sell = regime_block_sell + 1
        want_sell = 0
    end
    if block_buy and want_buy > 0 then
        regime_block_buy = regime_block_buy + 1
        want_buy = 0
    end
    -- 趋势过滤(2026-09-18 实测最有效的一项): 价 < SMA(n) 时禁止买入(不接飞刀)。
    if want_buy > 0 and trend_filter_sma > 0 then
        local sma_v = ctx:sma(pair, trend_filter_sma)
        if sma_v and price < sma_v then
            want_buy = 0
            skip_trend = skip_trend + 1
        end
    end
    local pos = ctx:position_size(pair) or 0
    if want_sell > 0 and want_sell > pos then
        -- 需要卖但真实仓位不够 → 不下单; **此时才**按 10 万在当前位置整本重算账本(防虚拟仓位扩张),
        -- 并把平衡价记为当前市价(用户 2026-09-18 规则)。
        sell_short = sell_short + 1
        v_reset(ctx, price)
        balance_price = price
        need_rehang = true
        want_sell = 0
        if sell_short == 1 or sell_short % 50 == 0 then
            ctx:log(string.format(
                "[shannon_etf_accum] 网格卖侧卖出不足(累计 %d 次): 真实持仓 %.6f < 应卖 %.6f " ..
                "→ 零下单, 账本按 10 万整本重算(v_coin %.6f, 锚 := %.4f)",
                sell_short, pos, want_sell, v_coin, balance_price))
        end
    end
    local buy_size = cap_size(ctx, "buy", want_buy, buy_px, fee_bps)
    local sell_size = cap_size(ctx, "sell", want_sell, sell_px, fee_bps)
    if want_buy > 0 and buy_size < want_buy then
        buy_capped = buy_capped + 1 -- 现金不足被截断
    end

    local buy_ok = buy_size > 0 and buy_size * buy_px >= min_notional
    local sell_ok = sell_size > 0 and sell_size * sell_px >= min_notional
    if not buy_ok then
        if want_buy > 0 then
            skip_notional = skip_notional + 1
        end
        buy_size = 0
    end
    if not sell_ok then
        if want_sell > 0 then
            skip_notional = skip_notional + 1
        end
        sell_size = 0
    end
    last_rehang_ts = now_ts
    need_rehang = false
    if not buy_ok and not sell_ok then
        -- 判据拦下的一侧必须撤掉旧挂单: 否则牛市里那张旧卖单会留在盘上并成交 —— 规则被违反
        -- (2026-09-18 架构审核 P3)。名义不足等其余情况沿用原行为(不额外撤单)。
        if block_buy or block_sell then
            regime_blocked_cancel = regime_blocked_cancel + 1
            return { { pair = pair, action = "cancel_pending" } }
        end
        if skip_notional == 1 or skip_notional % 120 == 0 then
            ctx:log(string.format(
                "[shannon_etf_accum] 双边名义均 < min_notional=%.2f(累计跳过 %d 次), 本轮不挂单",
                min_notional, skip_notional))
        end
        return {}
    end

    rehang_count = rehang_count + 1
    local orders = { { pair = pair, action = "cancel_pending" } }
    if buy_ok then
        orders[#orders + 1] = {
            pair = pair, side = "buy", size = buy_size, price = buy_px, order_type = buy_type,
        }
    end
    if sell_ok then
        orders[#orders + 1] = {
            pair = pair, side = "sell", size = sell_size, price = sell_px, order_type = sell_type,
        }
    end
    if rehang_count <= 3 or rehang_count % 24 == 0 then
        ctx:log(string.format(
            "[shannon_etf_accum] 重挂 #%d: 锚=%.4f atr=%.4f spacing=%.4f 买 %.6f@%.4f(%s) 卖 %.6f@%.4f(%s)",
            rehang_count, balance_price, atr, spacing, buy_size, buy_px, buy_type, sell_size, sell_px,
            sell_type))
    end
    return orders
end

function on_fill(ctx, fill)
    built = true
    -- 成交后: 锚 := 成交价; 虚拟账本**按成交本身继续演化**(买: coin+size, cash−size×价; 卖反之)。
    -- 用户口径(2026-09-18): "买入不按 10w 重新计算, 而是按现有虚拟仓位继续计算";
    -- 只有"卖出但真实仓位没有/不够"时才按 10 万重算账本(R3 与网格卖侧两处)。
    balance_price = fill.fill_price
    local size = fill.fill_size or 0
    local px = fill.fill_price or 0
    if fill.side == "buy" then
        v_coin = v_coin + size
        v_cash = v_cash - size * px
    else
        v_coin = v_coin - size
        v_cash = v_cash + size * px
    end
    last_side = fill.side
    need_rehang = true
    fill_count = fill_count + 1
    -- 网格成交 = 全部成交 − 交叉通道成交(OrderFill 不携带 order_type, 用派生口径最可靠)。
    grid_fills = fill_count - cross_buy - cross_sell_real
    -- Task 7: 逐笔状态行(供还原"多仓逐步增加"曲线)
    log_state(ctx, string.format("成交 #%d %s %.6f @ %.4f →",
        fill_count, fill.side, fill.fill_size, fill.fill_price))
end

function on_stop(ctx)
    log_state(ctx, "收尾")
    if cfg_str(ctx, "regime_filter", "off") == "ema200" then
        ctx:log(string.format(
            "[shannon_etf_accum] 趋势判据统计: 末态 %s / 切换 %d 次 / tick 数 BULL %d RANGE %d " ..
            "BEAR %d / 拦买 %d 拦卖 %d 被拦撤单 %d / 判据未就绪跳过 %d",
            string.upper(regime_state), regime_switch, ticks_bull, ticks_range, ticks_bear,
            regime_block_buy, regime_block_sell, regime_blocked_cancel, regime_ready_skip))
    end
    ctx:log(string.format(
        "[shannon_etf_accum] 停机: 成交 %d(网格 %d) / 重挂 %d / 交叉(买 %d, 真卖 %d, 只记账 %d) / " ..
        "跳过(ATR未就绪 %d, 低间距 %d, 小名义 %d, 趋势过滤 %d) / 受限(卖出仓位不足 %d, 买入现金截断 %d, 熊市禁买 %d) / " ..
        "金叉检出 %d 死叉检出 %d / 末次成交 %s / " ..
        "账本末态(币 %.6f 现金 %.0f 权益 %.0f) 配置倍数 %.1f 有效倍数 %.3f",
        fill_count, grid_fills, rehang_count, cross_buy, cross_sell_real, cross_sell_virtual,
        skip_no_atr, skip_thin, skip_notional, skip_trend, sell_short, buy_capped, regime_blocked, cross_golden_seen,
        cross_death_seen, last_side or "无",
        v_coin, v_cash, v_coin * last_price + v_cash, num(ctx, "leverage_mult", 10),
        equity_val > 0 and (v_coin * last_price + v_cash) / equity_val or 0))
end
