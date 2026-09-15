-- 合约做多冒烟示例 (USDT-M 永续, one-way)。
-- 用法: ricow backtest --strategy lua --script examples/futures_long.lua \
--          --pair BTCUSDT --days 14 --interval 1h --market futures
--
-- 演示: ①方向仓查询 ctx:pos_size(pair, "long") ②position_side 订单字段
--       ③reduce_only 平多 (sell + position_side="long" + reduce_only)。
-- 行情经已收盘 K 线指标 (无前视); 数量 0.001 BTC 适配 ~$80k 价格 (名义 ~$80, 现金充裕)。

function on_tick(ctx)
    local pair = ctx:config_str("pair")
    if pair == "" then pair = "BTCUSDT" end
    local fast = ctx:ema(pair, 5)
    local slow = ctx:ema(pair, 20)
    if not fast or not slow then return {} end  -- 数据不足, 等更多已收盘 K 线

    local long = ctx:pos_size(pair, "long")
    local orders = {}
    if fast > slow and long == 0 then
        -- 金叉: 开多 (position_side 指定作用方向仓; 合约 1x 默认, 现金足够)。
        orders[#orders + 1] = {
            pair = pair, side = "buy", size = 0.001, order_type = "market",
            position_side = "long",
        }
    elseif fast < slow and long > 0 then
        -- 死叉: 平多 (reduce_only 防翻转)。
        orders[#orders + 1] = {
            pair = pair, side = "sell", size = long, order_type = "market",
            position_side = "long", reduce_only = true,
        }
    end
    return orders
end
