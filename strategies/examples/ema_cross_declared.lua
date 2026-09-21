-- 028 样板: 声明驱动的双均线策略 (策略自己决定数据来源/标的/周期/节奏)。
--
-- 与旧写法的区别(这就是 028 的迁移点):
--   旧: 引擎写死喂 `config.pair` 的 K 线 → 策略只能"填空", 数据/周期/窗口全由引擎定。
--   新: 策略在**脚本顶层**声明要哪条序列(`data:series`) + 是否要引擎驱动(`drive=true`),
--       引擎负责装载(含预热)、按 `close_time` 逐根推送, 策略在 `on_bar` 里读句柄指标。
--
-- 跑法(数据先落本地库; 回测只读本地库, 不联网 —— 可复现):
--   ricow data pull --source binance_spot --symbol ETHUSDT --interval 1h --days 150
--   ricow backtest --strategy lua --script strategies/examples/ema_cross_declared.lua \
--       --pair ETHUSDT --days 60 --cash 10000
--
-- 声明参数:
--   source/symbol/interval = 数据来源与周期(第三方源同理, 如 source="nasdaq" symbol="QQQ" interval="1d")
--   bars    = 策略可见尾窗根数(句柄最多保留这么多根)
--   min_bars= 预热下限(不足直接报"序列过短", 不会拿半截指标做决策)
--   drive   = true → 引擎按该序列收盘回调 `on_bar`(回测里它同时是主时钟)
--
-- 两条写法纪律(实盘/DryRun/回测三端同一份脚本都靠它们):
--   1. 仓况以 `ctx:pos_size` 真值为准, 不用脚本内记忆 —— 下单可能不成交。
--   2. 买单留费率/滑点余量, 否则满额下单会被"资金不足"拒掉(实测踩过)。

local eth = data:series{
    id = "eth1h", source = "binance_spot", symbol = "ETHUSDT", interval = "1h",
    bars = 300, min_bars = 60, drive = true,
}

local PAIR = "ETHUSDT"

function on_bar(ctx, series, bar)
    local fast = eth:ema(12)
    local slow = eth:ema(48)
    if fast == nil or slow == nil then
        return {}
    end

    local sz = ctx:pos_size(PAIR, "long") or 0
    local want_long = fast > slow

    -- 空翻多: 按现金建多仓(留 2% 余量给费与滑点)。
    if want_long and sz <= 0 then
        local cash = ctx:balance("USDT") or 0
        local px = bar.close
        if cash > 0 and px > 0 then
            return { { pair = PAIR, side = "buy", size = (cash * 0.98) / px, order_type = "market" } }
        end
    -- 多翻空: 平掉全部多头。
    elseif (not want_long) and sz > 0 then
        return { { pair = PAIR, side = "sell", size = sz, order_type = "market", reduce_only = true } }
    end
    return {}
end
