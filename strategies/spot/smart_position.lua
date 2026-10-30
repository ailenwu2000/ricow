-- 现货智能持仓 (smart_position) — 多标的组合
-- 用户向说明见同名清单 strategies/spot/smart_position.toml; 参数含义见其 [[params]]
quote_asset = ""

-- 参数生效值(on_init 定格)
TARGET = 0.5
CYCLE_SEC = 86400
THRESH_PP = 2
EARN_ON = true
BUFFER_PCT = 0.5
APR = 0.02
MIN_NOTIONAL = 5
FEE = 0.001
UNIVERSE = {}            -- 币列表
WEIGHTS = {}             -- 与 UNIVERSE 一一对应

-- 周期与状态
last_cycle_ts = nil
last_tick_ts = nil
halted = false
fatal = 0
earn_supported = nil       -- nil=未探测; false=理财不可用(静默降级)
equity0 = 0                -- 首次执行时权益(利息估算与统计基准)

-- 计数(可观测性)
rebalance_count = 0
buy_count = 0
sell_count = 0
pending_redeem_count = 0
dev_max_pp = 0

-- 理财计时(USDT·秒): 利息估算 = 计时/86400 × APR / 365
earn_time_sum = 0

-- 策略账本(与引擎交叉核对): 现金含理财划转意图, 成交在 on_fill 入账
m_cash = nil
m_pos = {}                 -- 键 = pair
m_earn = 0
m_fee = 0

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

-- 逗号串 → 列表(去空白、跳空项)
local function split_csv(s)
    local out = {}
    if s == nil or s == "" then
        return out
    end
    for item in string.gmatch(s, "[^,]+") do
        local t = item:match("^%s*(.-)%s*$")
        if t ~= "" then
            table.insert(out, t)
        end
    end
    return out
end

function save_state(ctx)
    if last_cycle_ts ~= nil then
        ctx:state_set("last_cycle_ts", string.format("%d", math.floor(last_cycle_ts)))
    end
    ctx:state_set("rebalance_count", string.format("%d", rebalance_count))
    ctx:state_set("pending_redeem_count", string.format("%d", pending_redeem_count))
    ctx:state_set("buy_count", string.format("%d", buy_count))
    ctx:state_set("sell_count", string.format("%d", sell_count))
    ctx:state_set("dev_max_pp", string.format("%.4f", dev_max_pp))
    ctx:state_set("earn_time_sum", string.format("%.2f", earn_time_sum))
    ctx:state_set("equity0", string.format("%.2f", equity0))
    ctx:state_set("m_fee", string.format("%.4f", m_fee))
    ctx:state_set("halted", halted and "1" or "0")
end

function on_init(ctx)
    UNIVERSE = split_csv(ctx:config_str("universe"))
    if #UNIVERSE == 0 then
        local pair = ctx:config_str("pair")
        if pair ~= nil and pair ~= "" then
            UNIVERSE = { pair }
        end
    end
    if #UNIVERSE == 0 then
        halted = true
        fatal = 1
        ctx:log("[smart_position] [FATAL] universe/pair 均为空 -> 停机")
        return
    end
    -- 全池共用一个报价币: universe 各项报价必须一致, 混报价(如 BTCUSDT,ETHUSDC)即停机。
    quote_asset = exec.detect_quote(UNIVERSE[1])
    for _, p in ipairs(UNIVERSE) do
        if exec.detect_quote(p) ~= quote_asset then
            halted = true
            fatal = 1
            ctx:log(string.format(
                "[smart_position] [FATAL] universe 报价币不一致: %s(%s) 与 %s(%s) -> 停机",
                UNIVERSE[1], quote_asset, p, exec.detect_quote(p)))
            return
        end
    end

    -- weights: 缺省 = 等权; 显式给出则严格校验(个数/范围/合计), 不静默归一。
    local wstr = ctx:config_str("weights")
    if wstr == nil or wstr == "" then
        local w = 1.0 / #UNIVERSE
        WEIGHTS = {}
        for _ = 1, #UNIVERSE do
            table.insert(WEIGHTS, w)
        end
    else
        WEIGHTS = {}
        for _, tok in ipairs(split_csv(wstr)) do
            table.insert(WEIGHTS, tonumber(tok))
        end
    end

    local main_intv = cfg_str(ctx, "interval", "1h")
    ctx:need_klines("primary", main_intv, 24)

    -- target_pct/earn_enabled/earn_apr 可合法取 0, 不能走 num()(0 视作未配置);
    -- config_f64 缺键返回 0 而非 nil, 缺省值由清单 [[params]] default 注入生效配置保证。
    TARGET = ctx:config_f64("target_pct")
    CYCLE_SEC = num(ctx, "cycle_days", 1) * 86400
    THRESH_PP = num(ctx, "rebalance_threshold_pct", 2)
    EARN_ON = ctx:config_f64("earn_enabled") >= 0.5
    BUFFER_PCT = num(ctx, "earn_buffer_pct", 0.5)
    APR = ctx:config_f64("earn_apr")
    MIN_NOTIONAL = num(ctx, "min_notional", 5)
    FEE = num(ctx, "fee_side", 0.001)

    local bad
    if TARGET < 0 or TARGET > 1 then
        bad = string.format("target_pct=%.4f 须在 [0, 1]", TARGET)
    elseif CYCLE_SEC <= 0 then
        bad = "cycle_days 须 > 0"
    elseif THRESH_PP < 0 then
        bad = "rebalance_threshold_pct 须 ≥ 0"
    elseif #WEIGHTS ~= #UNIVERSE then
        bad = string.format("weights 个数 %d 与 universe 个数 %d 不一致", #WEIGHTS, #UNIVERSE)
    else
        local sum = 0
        for i, w in ipairs(WEIGHTS) do
            if w == nil or w ~= w or w < 0 or w > 1 then
                bad = string.format("weights[%d]=%s 须在 [0, 1]", i, tostring(w))
                break
            end
            sum = sum + w
        end
        if bad == nil and math.abs(sum - 1) > 1e-6 then
            bad = string.format("weights 合计 %.8f 须 ≈ 1 (不自动归一, 请修正配置)", sum)
        end
    end
    if bad then
        halted = true
        fatal = 1
        ctx:log("[smart_position] [FATAL] 参数校验失败 -> 停机: " .. bad)
        return
    end

    -- 断点续接: 恢复上次执行时刻与计数, 避免重启即调仓(防循环)。
    local ts = ctx:state_get("last_cycle_ts")
    if ts then
        last_cycle_ts = tonumber(ts)
    end
    local rc = ctx:state_get("rebalance_count")
    if rc then
        rebalance_count = tonumber(rc) or 0
    end
    local pr = ctx:state_get("pending_redeem_count")
    if pr then
        pending_redeem_count = tonumber(pr) or 0
    end
    local bc = ctx:state_get("buy_count")
    if bc then
        buy_count = tonumber(bc) or 0
    end
    local sc = ctx:state_get("sell_count")
    if sc then
        sell_count = tonumber(sc) or 0
    end
    local dm = ctx:state_get("dev_max_pp")
    if dm then
        dev_max_pp = tonumber(dm) or 0
    end
    local ets = ctx:state_get("earn_time_sum")
    if ets then
        earn_time_sum = tonumber(ets) or 0
    end
    local e0 = ctx:state_get("equity0")
    if e0 then
        equity0 = tonumber(e0) or 0
    end
    local mf = ctx:state_get("m_fee")
    if mf then
        m_fee = tonumber(mf) or 0
    end
    local h = ctx:state_get("halted")
    if h == "1" then
        halted = true
        fatal = 1
    end

    local parts = {}
    for i, p in ipairs(UNIVERSE) do
        table.insert(parts, string.format("%s:%.4f", p, WEIGHTS[i]))
    end
    ctx:log(string.format(
        "[smart_position] 启动: universe=[%s] target=%.2f cycle=%.3fd 阈值=%.2fpp 理财=%s 缓冲=%.2f%% min_notional=%.2f",
        table.concat(parts, ", "), TARGET, CYCLE_SEC / 86400, THRESH_PP,
        EARN_ON and "on" or "off", BUFFER_PCT, MIN_NOTIONAL))
end

function on_tick(ctx)
    if halted then
        return {}
    end
    local t = ctx:now()
    if t == nil then
        return {}
    end

    -- 理财余额探测: nil = 该资产不支持理财 → 静默关闭理财腿, 不报错。
    local earn = ctx:earn_balance(quote_asset)
    if earn == nil then
        earn_supported = false
        earn = 0
    else
        earn_supported = true
    end

    -- 理财计时: 每 tick 按距上次的秒数累计"在理财 USDT·秒"(利息估算用)。
    if last_tick_ts ~= nil then
        local dt = t.ts - last_tick_ts
        if dt > 0 then
            earn_time_sum = earn_time_sum + earn * dt
        end
    end
    last_tick_ts = t.ts

    -- 时间闸: 首 tick 立即执行; 之后每 cycle_days 一次。
    if last_cycle_ts ~= nil and t.ts - last_cycle_ts < CYCLE_SEC then
        return {}
    end

    -- 逐币现价/持仓快照 + 权益(现金 + Σ持仓市值 + 理财)。
    local cash = ctx:balance(quote_asset) or 0
    local prices, poss, holdings_sum = {}, {}, 0
    for _, pair in ipairs(UNIVERSE) do
        local p = ctx:price(pair)
        prices[pair] = (p ~= nil and p > 0) and p or nil
        poss[pair] = ctx:position_size(pair) or 0
        if prices[pair] ~= nil then
            holdings_sum = holdings_sum + poss[pair] * prices[pair]
        end
    end
    local equity = cash + holdings_sum + earn
    if equity <= 0 then
        ctx:log("[smart_position] WARN: 权益为 0, 本周期跳过(无资金可配置)")
        return {}
    end
    if equity0 <= 0 then
        equity0 = equity
        m_cash = cash
        for _, pair in ipairs(UNIVERSE) do
            m_pos[pair] = poss[pair]
        end
    end

    last_cycle_ts = t.ts
    local buy_emitted = false
    local sells, buys = {}, {}

    -- 阈值闸逐币独立: 只调越界的币。先收集卖腿(回笼资金)再收集买腿(消耗资金)。
    for i, pair in ipairs(UNIVERSE) do
        local price = prices[pair]
        if price ~= nil then
            local target_share = TARGET * WEIGHTS[i]
            local cur = poss[pair] * price / equity
            local dev_pp = (cur - target_share) * 100
            if math.abs(dev_pp) > dev_max_pp then
                dev_max_pp = math.abs(dev_pp)
            end
            if math.abs(dev_pp) >= THRESH_PP then
                local delta = (target_share - cur) * equity   -- >0 买入, <0 卖出
                if delta > 0 then
                    table.insert(buys, { pair = pair, price = price, delta = delta, target_share = target_share })
                else
                    table.insert(sells, { pair = pair, price = price, delta = delta, target_share = target_share })
                end
            end
        end
    end

    local orders = {}
    local any_redeem = false
    local sell_emitted = false
    -- 卖腿优先: 卖出所得滚动进本周期买腿预算(引擎按下单序即时撮合, 先卖后买)。
    -- 市价单含费修正: 卖 size = Δ/(p×(1−f)), 成交净额 ≈ Δ 到位。
    for _, s in ipairs(sells) do
        local size = (-s.delta) / (s.price * (1 - FEE))
        local cap = poss[s.pair] * (1 - 1e-9)
        if size > cap then
            size = cap
        end
        if size * s.price >= MIN_NOTIONAL and size > 0 then
            table.insert(orders, { pair = s.pair, side = "sell", size = size, order_type = "market" })
            sell_emitted = true
            rebalance_count = rebalance_count + 1
            cash = cash + size * s.price * (1 - FEE)
            ctx:log(string.format(
                "[smart_position] 调仓卖出 %s: cur=%.4f%% target=%.4f%% size=%.6f 名义=%.2f",
                s.pair, (poss[s.pair] * s.price / equity) * 100, s.target_share * 100, size, size * s.price))
        end
    end
    -- 买腿: 按滚动现金预算; 不足 → 赎回差额申请、本周期跳过该币(下周期再试)。
    -- 含费修正: 买 size = Δ/(p×(1+f)), 总支出(本金+费) ≈ Δ。
    for _, b in ipairs(buys) do
        local size = b.delta / (b.price * (1 + FEE))
        local cost = size * b.price
        -- 本金+手续费(引擎现金支出口径), 预留 1bp 余量: 引擎费用按 Decimal 舍入,
        -- f64 预算恰好打平时会被"资金不足"拒单。
        local need = cost * (1 + FEE) * (1 + 1e-4)
        local did_redeem = false
        if need > cash + 1e-6 then
            local shortfall = need - cash
            if earn > 0 then
                local redeem_amt = math.min(earn, shortfall)
                if ctx:earn_redeem(quote_asset, redeem_amt) then
                    m_cash = m_cash + redeem_amt
                    m_earn = m_earn - redeem_amt
                    cash = cash + redeem_amt
                    earn = earn - redeem_amt
                end
                did_redeem = true
                any_redeem = true
                pending_redeem_count = pending_redeem_count + 1
                ctx:log(string.format(
                    "[smart_position] %s 现金不足 %.2f, 已提交赎回申请 %.2f, 本周期不调该币",
                    b.pair, shortfall, redeem_amt))
            else
                size = cash / (b.price * (1 + FEE) * (1 + 1e-4))
                cost = size * b.price
            end
        end
        if not did_redeem and cost >= MIN_NOTIONAL and size > 0 then
            table.insert(orders, { pair = b.pair, side = "buy", size = size, order_type = "market" })
            buy_emitted = true
            rebalance_count = rebalance_count + 1
            cash = cash - cost * (1 + FEE)
            ctx:log(string.format(
                "[smart_position] 调仓买入 %s: cur=%.4f%% target=%.4f%% size=%.6f 名义=%.2f",
                b.pair, (poss[b.pair] * b.price / equity) * 100, b.target_share * 100, size, cost))
        end
    end

    -- 理财管理: 现金腿超缓冲部分申购; 本周期有赎回/买单/卖单时不动(下周期自愈)。
    -- 卖单同周期不申购: 理财划转在成交前执行, 卖出所得尚未落账, 按滚动现金申购会被拒。
    if EARN_ON and earn_supported and not any_redeem and not buy_emitted and not sell_emitted then
        local buffer = equity * BUFFER_PCT / 100
        local excess = cash - buffer
        if excess >= MIN_NOTIONAL then
            if ctx:earn_subscribe(quote_asset, excess) then
                m_cash = m_cash - excess
                m_earn = m_earn + excess
                ctx:log(string.format(
                    "[smart_position] 闲置现金入理财: %.2f (缓冲保留 %.2f)", excess, buffer))
            end
        end
    end

    save_state(ctx)
    return orders
end

function on_fill(ctx, fill)
    if fill.client_order_id:find("LIQ-") or fill.client_order_id:find("CLOSE-") then
        return {}
    end
    local px = fill.fill_price
    local sz = fill.fill_size
    local fee = fill.fee or 0
    m_fee = m_fee + fee
    local pair = fill.pair
    if m_pos[pair] == nil then
        m_pos[pair] = ctx:position_size(pair) or 0
    end
    if fill.side == "buy" then
        buy_count = buy_count + 1
        m_cash = m_cash - px * sz - fee
        m_pos[pair] = m_pos[pair] + sz
    else
        sell_count = sell_count + 1
        m_cash = m_cash + px * sz - fee
        m_pos[pair] = m_pos[pair] - sz
    end
    return {}
end

function on_stop(ctx)
    local cash = ctx:balance(quote_asset) or 0
    local earn = ctx:earn_balance(quote_asset) or 0
    local pos_str, price_str = {}, {}
    local holdings = 0
    for _, pair in ipairs(UNIVERSE) do
        local p = ctx:position_size(pair) or 0
        local px = ctx:price(pair) or 0
        holdings = holdings + p * px
        table.insert(pos_str, string.format("%s=%.8f", pair, p))
        table.insert(price_str, string.format("%s=%.6f", pair, px))
    end
    local equity = cash + holdings + earn

    -- 账本交叉核对: 策略账本(成交+划转意图) vs 引擎实际(现货现金/各币持仓/理财)。
    local d_cash = (m_cash or cash) - cash
    local d_earn = m_earn - earn
    local d_pos_max = 0
    for _, pair in ipairs(UNIVERSE) do
        local d = (m_pos[pair] or 0) - (ctx:position_size(pair) or 0)
        if math.abs(d) > math.abs(d_pos_max) then
            d_pos_max = d
        end
    end
    if math.abs(d_cash) > 0.01 or math.abs(d_pos_max) > 1e-8 or math.abs(d_earn) > 0.01 then
        ctx:log(string.format(
            "[smart_position] WARN 账本核对偏差: cash=%.6f pos=%.8f earn=%.6f", d_cash, d_pos_max, d_earn))
    end

    local earn_days = earn_time_sum / 86400
    local interest_est = earn_days * APR / 365

    local uni_parts, w_parts = {}, {}
    for i, p in ipairs(UNIVERSE) do
        table.insert(uni_parts, p)
        table.insert(w_parts, string.format("%.4f", WEIGHTS[i] or 0))
    end
    ctx:state_set("stat_universe", table.concat(uni_parts, ","))
    ctx:state_set("stat_weights", table.concat(w_parts, ","))
    ctx:state_set("stat_target_pct", string.format("%.4f", TARGET))
    ctx:state_set("stat_cycle_days", string.format("%.3f", CYCLE_SEC / 86400))
    ctx:state_set("stat_threshold_pp", string.format("%.4f", THRESH_PP))
    ctx:state_set("stat_earn_enabled", EARN_ON and "1" or "0")
    ctx:state_set("stat_earn_supported", earn_supported and "1" or "0")
    ctx:state_set("stat_earn_buffer_pct", string.format("%.4f", BUFFER_PCT))
    ctx:state_set("stat_earn_apr", string.format("%.4f", APR))
    ctx:state_set("stat_min_notional", string.format("%.2f", MIN_NOTIONAL))
    ctx:state_set("stat_fee_side", string.format("%.4f", FEE))
    ctx:state_set("stat_rebalance_count", string.format("%d", rebalance_count))
    ctx:state_set("stat_buy_count", string.format("%d", buy_count))
    ctx:state_set("stat_sell_count", string.format("%d", sell_count))
    ctx:state_set("stat_pending_redeem_count", string.format("%d", pending_redeem_count))
    ctx:state_set("stat_dev_max_pp", string.format("%.4f", dev_max_pp))
    ctx:state_set("stat_earn_usdt_days", string.format("%.2f", earn_days))
    ctx:state_set("stat_earn_interest_est", string.format("%.4f", interest_est))
    ctx:state_set("stat_earn_final", string.format("%.2f", earn))
    ctx:state_set("stat_equity_final", string.format("%.2f", equity))
    ctx:state_set("stat_equity0", string.format("%.2f", equity0))
    ctx:state_set("stat_cash_final", string.format("%.2f", cash))
    ctx:state_set("stat_pos_final", table.concat(pos_str, ";"))
    ctx:state_set("stat_price_final", table.concat(price_str, ";"))
    ctx:state_set("stat_fee_total", string.format("%.4f", m_fee))
    ctx:state_set("stat_ledger_diff_cash", string.format("%.6f", d_cash))
    ctx:state_set("stat_ledger_diff_pos", string.format("%.8f", d_pos_max))
    ctx:state_set("stat_ledger_diff_earn", string.format("%.6f", d_earn))
    ctx:state_set("stat_halted", halted and "1" or "0")

    ctx:log(string.format(
        "[smart_position] 停机: 调仓%d次(买%d/卖%d) 赎回申请%d次 最大偏离%.2fpp 期末权益%.2f(理财%.2f) 利息估算%.4f",
        rebalance_count, buy_count, sell_count, pending_redeem_count, dev_max_pp, equity, earn, interest_est))
end
