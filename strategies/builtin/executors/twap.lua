-- TWAP 时间加权分批执行模式示例 (非策略: 最简信号 = 按片定时触发 + exec.* 执行)。
-- 参数: pair / total_size / num_slices / slice_interval_secs / bar_seconds(CLI 按 interval 注入) / side(默认 buy)。
-- 用法: ricow backtest --strategy twap --pair ETH

pair = nil
total_size = 0
num_slices = 0
slice_size = 0
ticks_per_slice = 1
side = "buy"
start_tick = nil
slices_sent = 0
done_sending = false

function on_init(ctx)
    pair = ctx:config_str("pair")
    total_size = ctx:config_f64("total_size")
    num_slices = ctx:config_i64("num_slices")
    local interval = ctx:config_i64("slice_interval_secs")
    local bar_sec = ctx:config_i64("bar_seconds")
    ticks_per_slice = exec.ticks_per(interval, bar_sec)
    local s = ctx:config_str("side")
    if s == "sell" then side = "sell" end
    if num_slices >= 1 then
        slice_size = total_size / num_slices
    end
    start_tick = nil
    slices_sent = 0
    done_sending = false
    ctx:log("twap: 初始化 " .. pair .. " total=" .. total_size .. " 分 " .. num_slices
        .. " 片 每片=" .. slice_size .. " 间隔 " .. ticks_per_slice .. " tick")
end

-- 本片是否到期: done 由本函数先判, 到期判断交 exec.slice_due (首片立即, 之后每 ticks_per_slice 一片)。
function slice_due()
    if done_sending then
        return false
    end
    return exec.slice_due(tick_count, start_tick, slices_sent, ticks_per_slice)
end

tick_count = 0

function on_tick(ctx)
    tick_count = tick_count + 1
    if not pair or pair == "" or num_slices < 1 or total_size <= 0 then
        return {}
    end
    if start_tick == nil then
        start_tick = tick_count
    end
    if not slice_due() then
        return {}
    end

    -- 对手价限价 (买 best_ask / 卖 best_bid), 无盘口回退市价。
    local order = exec.side_order(ctx, pair, side, slice_size)

    slices_sent = slices_sent + 1
    if slices_sent >= num_slices then
        done_sending = true
    end
    ctx:log("twap: 第 " .. slices_sent .. "/" .. num_slices .. " 片发出")
    return { order }
end

function on_fill(ctx, fill)
    if fill.pair == pair and fill.side == side then
        ctx:log("twap: 第 " .. slices_sent .. " 片成交")
    end
end
