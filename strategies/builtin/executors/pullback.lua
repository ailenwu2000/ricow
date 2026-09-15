-- 限价回调执行模式示例 (非策略: 最简信号 = 新高后回撤触发 + exec.* 执行, 改信号即自定义策略)。
-- 做多: 价格创新高(HWM)后回撤 pullback_pct / pullback_abs 时买入; 做空对称。
-- 参数: pair / side(默认 buy) / activation_price(可选) / pullback_pct / pullback_abs(二选一) / order_size。
-- 用法: ricow backtest --strategy pullback --pair ETH

pair = nil
side = "buy"
activation_price = nil
pullback_pct = nil
pullback_abs = nil
order_size = 0
extreme = nil
triggered = false
done = false

-- 回调触发判断在 exec.pullback_triggered (纯函数, 参数化 side/pct/abs)。

function on_init(ctx)
    pair = ctx:config_str("pair")
    order_size = ctx:config_f64("order_size")
    local s = ctx:config_str("side")
    if s == "sell" then side = "sell" end
    local act = ctx:config_f64("activation_price")
    if act > 0 then activation_price = act end
    local pct = ctx:config_f64("pullback_pct")
    if pct > 0 and pct < 1 then pullback_pct = pct end
    local abs = ctx:config_f64("pullback_abs")
    if abs > 0 then pullback_abs = abs end
    extreme = nil
    triggered = false
    done = false
    ctx:log("pullback: 初始化 " .. pair .. " side=" .. side .. " 激活价=" .. tostring(activation_price)
        .. " 回调(pct=" .. tostring(pullback_pct) .. ", abs=" .. tostring(pullback_abs)
        .. ") 每笔=" .. order_size)
end

-- 限价单 (无盘口回退市价, 与 Rust 版一致): 订单构造在 exec.side_order。
function make_order(ctx)
    return exec.side_order(ctx, pair, side, order_size)
end

function on_tick(ctx)
    if done then
        return {}
    end
    local price = ctx:price(pair)
    if not price then
        return {}
    end

    -- 激活价: 做多需 price >= activation 才监控; 做空需 price <= activation。
    if activation_price then
        local active = (side == "buy" and price >= activation_price)
            or (side == "sell" and price <= activation_price)
        if not active then
            return {}
        end
    end

    -- 更新极值 (HWM / LWM)。
    if extreme == nil then
        extreme = price
    elseif side == "buy" then
        if price > extreme then extreme = price end
    else
        if price < extreme then extreme = price end
    end

    if not triggered and exec.pullback_triggered(side, price, extreme, pullback_pct, pullback_abs) then
        triggered = true
        ctx:log("pullback: " .. pair .. " 回调触发 @ " .. price)
        return { make_order(ctx) }
    end
    return {}
end

function on_fill(ctx, fill)
    if fill.pair == pair and fill.side == side then
        done = true
        ctx:log("pullback: " .. pair .. " 成交, 完成")
    end
end
