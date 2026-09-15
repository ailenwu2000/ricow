-- VWAP 成交量加权分批执行模式示例 (非策略: 最简信号 = 按片定时触发 + VWAP 参考价限价 + exec.* 执行)。
-- 参数: pair / total_size / num_slices / slice_interval_secs / bar_seconds(CLI 按 interval 注入)
--       / side(默认 buy) / lookback_bars(默认 20)。
-- 参考价 = 最近 lookback_bars 根已收盘 K 线的成交量加权均价 (无前视);
-- 无历史 K 线或成交量全 0 时回退市价。
-- 用法: ricow backtest --strategy vwap --pair ETH

pair = nil
total_size = 0
num_slices = 0
slice_size = 0
ticks_per_slice = 1
side = "buy"
lookback_bars = 20
start_tick = nil
slices_sent = 0
done_sending = false
tick_count = 0

function on_init(ctx)
    pair = ctx:config_str("pair")
    total_size = ctx:config_f64("total_size")
    num_slices = ctx:config_i64("num_slices")
    local interval = ctx:config_i64("slice_interval_secs")
    local bar_sec = ctx:config_i64("bar_seconds")
    ticks_per_slice = exec.ticks_per(interval, bar_sec)
    local s = ctx:config_str("side")
    if s == "sell" then side = "sell" end
    local lb = ctx:config_i64("lookback_bars")
    if lb >= 1 then lookback_bars = lb end
    if num_slices >= 1 then
        slice_size = total_size / num_slices
    end
    start_tick = nil
    slices_sent = 0
    done_sending = false
    ctx:log("vwap: 初始化 " .. pair .. " total=" .. total_size .. " 分 " .. num_slices
        .. " 片 每片=" .. slice_size .. " 间隔 " .. ticks_per_slice .. " tick 窗口 " .. lookback_bars)
end

-- 成交量加权均价: 最近 lookback_bars 根已收盘 K 线 Σ(close×volume)/Σ(volume);
-- 无历史 K 线返回 nil; 有 K 线但成交量全 0 回退最新 close。
function vwap_ref(ctx)
    local bars = ctx:klines(pair)
    if not bars or #bars == 0 then
        return nil
    end
    local n = math.min(#bars, lookback_bars)
    local sum_pv = 0
    local sum_v = 0
    local last_close = nil
    for i = #bars - n + 1, #bars do
        local b = bars[i]
        last_close = b.close
        if b.volume and b.volume > 0 then
            sum_pv = sum_pv + b.close * b.volume
            sum_v = sum_v + b.volume
        end
    end
    if sum_v > 0 then
        return sum_pv / sum_v
    end
    return last_close
end

-- 本片是否到期: done 由本函数先判, 到期判断交 exec.slice_due (首片立即, 之后每 ticks_per_slice 一片)。
function slice_due()
    if done_sending then
        return false
    end
    return exec.slice_due(tick_count, start_tick, slices_sent, ticks_per_slice)
end

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

    -- 参考价限价单: VWAP 可算则挂 VWAP 价限价, 否则回退市价 (与 exec.side_order 的对手价语义不同, 故脚本内自行构造)。
    local ref = vwap_ref(ctx)
    local order = { pair = pair, side = side, size = slice_size, order_type = "market" }
    if ref then
        order.price = ref
        order.order_type = "limit"
    end

    slices_sent = slices_sent + 1
    if slices_sent >= num_slices then
        done_sending = true
    end
    ctx:log("vwap: 第 " .. slices_sent .. "/" .. num_slices .. " 片发出 参考价=" .. tostring(ref))
    return { order }
end

function on_fill(ctx, fill)
    if fill.pair == pair and fill.side == side then
        ctx:log("vwap: 第 " .. slices_sent .. " 片成交")
    end
end
