-- EMA 交叉策略 (lua-api.md 第五节示例): 快线上穿慢线开多, 下穿平多。
-- 用法: ricow backtest --strategy lua --script examples/ema_cross.lua --pair BNBUSDT
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
