-- 阶梯执行模式示例 (非策略: 一次性按档位挂限价单 + exec.* 执行, 对齐 Bybit Scaled Order)。
-- 参数: pair / side(默认 buy) / total_size / num_levels(2~100) / lower_price / upper_price
--       / distribution(equal 等差 / geometric 等比, 默认 equal)。
-- 用法: ricow backtest --strategy ladder --pair ETH

pair = nil
side = "buy"
num_levels = 0
level_size = 0
levels = {}
placed = false

-- 档位生成在 exec.levels (等差/等比, 与旧 generate_levels 一致)。

function on_init(ctx)
    pair = ctx:config_str("pair")
    total_size = ctx:config_f64("total_size")
    num_levels = ctx:config_i64("num_levels")
    local lower = ctx:config_f64("lower_price")
    local upper = ctx:config_f64("upper_price")
    local s = ctx:config_str("side")
    if s == "sell" then side = "sell" end
    local dist = ctx:config_str("distribution")
    local is_geo = (dist == "geometric" or dist == "geo")
    if num_levels >= 2 and num_levels <= 100 and total_size > 0 and upper > lower and lower > 0 then
        level_size = total_size / num_levels
        levels = exec.levels(lower, upper, num_levels, is_geo)
    end
    placed = false
    ctx:log("ladder: 初始化 " .. pair .. " side=" .. side .. " 总量=" .. total_size
        .. " 档数=" .. num_levels .. " 分布=" .. (is_geo and "geometric" or "equal")
        .. " [" .. lower .. " ~ " .. upper .. "] 每档=" .. level_size)
end

function on_tick(ctx)
    if placed or num_levels < 2 or level_size <= 0 then
        return {}
    end
    placed = true
    local orders = {}
    for _, price in ipairs(levels) do
        orders[#orders + 1] = {
            pair = pair, side = side, size = level_size, price = price, order_type = "limit"
        }
    end
    ctx:log("ladder: " .. pair .. " 挂 " .. #orders .. " 档限价单")
    return orders
end

function on_fill(ctx, fill)
    if fill.pair == pair and fill.side == side then
        ctx:log("ladder: " .. pair .. " 档位成交")
    end
end
