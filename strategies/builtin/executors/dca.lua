-- DCA 定时定投执行模式示例 (非策略: 最简信号 = 按间隔定时触发 + exec.* 执行, 改信号即自定义策略)。
-- 参数: pair / order_size / interval_secs / bar_seconds(CLI 按 interval 注入, 1h→3600) / max_buys。
-- 间隔换算在 exec.ticks_per, 回测/实盘均按 interval_secs 正常定投。
-- 用法: ricow backtest --strategy dca --pair ETH

pair = nil
order_size = 0
ticks_per_interval = 1
max_buys = nil
tick_count = 0
last_buy_tick = nil
buy_count = 0

function on_init(ctx)
    pair = ctx:config_str("pair")
    order_size = ctx:config_f64("order_size")
    ticks_per_interval = exec.ticks_per(ctx:config_i64("interval_secs"), ctx:config_i64("bar_seconds"))
    local mb = ctx:config_i64("max_buys")
    if mb > 0 then max_buys = mb end
    tick_count = 0
    last_buy_tick = nil
    buy_count = 0
    ctx:log("dca: 初始化 " .. pair .. " 每 " .. ticks_per_interval .. " tick 买入 " .. order_size)
end

function on_tick(ctx)
    tick_count = tick_count + 1
    if not pair or pair == "" or order_size <= 0 then
        return {}
    end
    if max_buys and buy_count >= max_buys then
        return {}
    end
    if last_buy_tick and tick_count - last_buy_tick < ticks_per_interval then
        return {}
    end
    if not ctx:price(pair) then
        return {}
    end
    last_buy_tick = tick_count
    ctx:log("dca: 执行第 " .. (buy_count + 1) .. " 次定投买入 " .. pair .. " " .. order_size)
    return { { pair = pair, side = "buy", size = order_size, order_type = "market" } }
end

function on_fill(ctx, fill)
    if fill.pair == pair and fill.side == "buy" then
        buy_count = buy_count + 1
        ctx:log("dca: 第 " .. buy_count .. " 次买入成交")
    end
end
