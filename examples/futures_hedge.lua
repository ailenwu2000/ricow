-- 合约对冲 (hedge) 冒烟示例 (USDT-M 永续, position_mode = hedge)。
-- 用法: ricow backtest --strategy lua --script examples/futures_hedge.lua \
--          --pair BTCUSDT --days 14 --interval 1h --market futures --position-mode hedge
--
-- 演示: ①hedge 双仓并存 (long/short 独立加仓与平仓, position_side 定向)
--       ②ctx:pos_size(pair, side) / ctx:pos_entry(pair, side) 方向查询
--       ③同一交易对多空可同时存在 (specs/backtest.md §五.6)。
-- 逻辑: 价格跌破布林下轨开多、升破上轨开空; 回到中轨两侧分别平仓 (均值回归)。
-- 参数: boll_dev 带宽 (默认 2σ; 冒烟调 1σ 增加触发概率)。

function on_tick(ctx)
    local pair = ctx:config_str("pair")
    if pair == "" then pair = "BTCUSDT" end
    local dev = ctx:config_f64("boll_dev")
    if dev <= 0 then dev = 2.0 end
    local b = ctx:boll(pair, 20, dev)
    local price = ctx:price(pair)
    if not b or not price then return {} end  -- 数据不足

    local long_sz = ctx:pos_size(pair, "long")
    local short_sz = ctx:pos_size(pair, "short")
    local orders = {}
    if price < b.lower then
        -- 超卖区: 开多 (不影响已存在的空头)。
        if long_sz == 0 then
            orders[#orders + 1] = {
                pair = pair, side = "buy", size = 0.001, order_type = "market",
                position_side = "long",
            }
        end
    elseif price > b.upper then
        -- 超买区: 开空 (不影响已存在的多头)。
        if short_sz == 0 then
            orders[#orders + 1] = {
                pair = pair, side = "sell", size = 0.001, order_type = "market",
                position_side = "short",
            }
        end
    else
        -- 中轨附近: 各自平仓 (buy + short 平空; sell + long 平多)。
        if long_sz > 0 then
            orders[#orders + 1] = {
                pair = pair, side = "sell", size = long_sz, order_type = "market",
                position_side = "long", reduce_only = true,
            }
        end
        if short_sz > 0 then
            orders[#orders + 1] = {
                pair = pair, side = "buy", size = short_sz, order_type = "market",
                position_side = "short", reduce_only = true,
            }
        end
    end
    return orders
end
