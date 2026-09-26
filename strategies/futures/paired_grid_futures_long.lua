-- 合约配对做多网格 (paired_grid_futures_long) -- v2 (2026-09-26, 用户指令: 砍空头改纯配对做多)
--
-- ═══ 一句话 ═══
--   USDT-M 合约(hedge, 只下 LONG 侧)配对做多网格: 行为逻辑与现货 paired_grid **完全一致** ——
--   价格低于 start_price 激活, 以最近成交价为参考价上下各挂一单(下方买单、上方平多卖单),
--   配对卖价恒 > 买价; flag(买 −1 / 卖 +1)驱动间距不对称放大; 成交即全撤重挂。
--
-- ═══ v1(双向) → v2(只做多) 设计变更 ═══
--   180d 回测(SOLUSDT 1m)证明空头侧是唯一亏损源(配对 +53.57 但期末强平锁亏 -160.83),
--   单边上涨行情中空头栈堆至 9 层、逼近爆仓价。用户指令砍掉空头侧:
--   只做多和平多, 逻辑与现货 strategies/spot/paired_grid.lua 逐行对齐。
--
-- ═══ 与现货 paired_grid 的差异(仅 5 点合约化) ═══
--   1) 挂单带 position_side="long"(hedge): 开多 buy+LONG / 平多 sell+LONG, 不带 reduce_only
--      (fapi 拒绝两者同带; 平多只减多头仓由 position_side 天然限定)。
--   2) cap_open 保证金模型: 名义/杠杆 ≤ 可用余额(预扣手续费+滑点余量), 替代现货纯现金约束。
--   3) 爆仓价监控: ctx:pos_liq(pair,"long") 距现价 < liq_warn_ratio 打 WARN(只告警, 不动作)。
--   4) 合约口径默认: min_notional 50 / fee_side 0.0005(taker 5bps)。
--   5) 期末强平统计: --close-at-end 的 CLOSE- fill 单列 close_pnl(持仓盈亏落袋), 与配对毛价差分开;
--      强平单可能跨多 lot, u 模式按 fill size 逐层扣减(正常卖单 size==栈顶, 与现货整 lot 出栈等价)。
--   其余(激活/建仓拆格/追踪/flag 放大/min_pair_profit 保底/coin 模式/成本门槛)与现货一字不差。
--   ⚠ 语义差异 1: 现货第 10 条「无持仓且无待配对仓 → 结束」**不移植**(用户 2026-09-26 口径:
--   "任何一单成交 → ref=成交价 → 全撤重挂"= 网格持续运行, 无结束条件); 合约版只做多,
--   平完栈后继续追踪+挂买, 长期运行。
--   ⚠ 语义差异 2 (2026-09-26 用户口径): **建仓不计算 flag** —— 建仓 lot(init) 被网格卖出时
--   不 flag+1(现货仍 +1)。否则建仓越厚 flag 冲得越高、卖单间距被放大到追不上行情,
--   成交随建仓名义增大反而锐减。
--   ⚠ 语义差异 3 (2026-09-26 用户口径): **卖价锚定栈顶买入成本**, 有仓位不随成交上移,
--   只随 flag 扩大间距; 栈空后才由追踪上移接管(现货仍锚 ref_price, 上涨段卖单逐笔爬升)。
--
-- ═══ 参数(全部可配, 默认值可直接回测) ═══
--   pair                必填, 合约对(如 SOLUSDT)
--   start_price         必填, 开始价格(价格低于它才激活); 缺失/≤0 启动停机
--   initial_buy_amount  初始建仓名义(USDT), 默认 0(=不建仓, 穿越即激活; 同现货口径)
--   interval            主时钟, 默认 1h
--   spacing_pct         网格间距(小数, 0.01=1%), 默认 0.01
--   order_amount        每格名义(USDT), 默认 10
--   direction_offset    方向偏移, 默认 0.2
--   accumulate_mode     成交模式, 默认 "u"(积累计价币); "coin"=积累币
--   min_notional        单笔最小名义, 默认 50(合约)
--   fee_side            单边费率, 默认 0.0005(合约 taker 5bps)
--   min_pair_profit     配对保底最小价差, 0→2×fee_side
--   leverage            保证金预算用(显示), 默认 2; 回测实际杠杆由引擎 [backtest].leverage 决定
--   liq_warn_ratio      距爆仓价告警阈值(小数), 默认 0.1(10%); 与引擎 014 的 liq_warn_pct(百分点)是两条通道
--
-- ═══ 用法 ═══
--   ricow backtest --strategy paired_grid_futures_long --pair SOLUSDT --interval 1m --cash 2000 \
--     --param start_price=<现价> --param initial_buy_amount=100 --param order_amount=100 --close-at-end
--   ricow run paired_grid_futures_long --demo   (直跑: market/position_mode/杠杆由清单回填)

quote_asset = ""  -- 计价资产, on_init 里 detect_quote(pair) 动态检测
-- 状态(单份, 与现货 paired_grid 变量一致)
built = false
ref_price = nil
flag = 0
flag_max = 0               -- flag 历史最大值(最多平多几个网格)
flag_min = 0               -- flag 历史最小值(负数, 最多买几个网格)
lots = {}                  -- LIFO 栈: 待配对买入仓, 每项 {qty=, price=}
init_lots = 0              -- 栈底建仓 lot 剩余笔数(2026-09-26 口径: 建仓及其平仓不计 flag)
pending_entry = false      -- 建仓市价单在途
need_rehang = false        -- 成交/追踪后置 true: 下一 tick 撤旧单 + 重挂
halted = false             -- 成本门槛/缺参数 → 停机
fatal = 0                  -- 数值化停机标记(单测经 global_f64 读取)
cost_checked = false       -- 成本门槛只做一次启动校验
last_bar_ts = nil
liq_warned = false         -- 爆仓价 WARN 去重
-- 计数(可观测性)
fill_count = 0
buy_count = 0
sell_count = 0
skip_notional = 0
rehang_count = 0
track_up_count = 0
last_price = 0
coin_accumulated = 0       -- coin 模式配对积累的币
quote_accumulated = 0      -- u 模式配对积累的价差(毛额, 未扣手续费)
fee_acc = 0                -- 手续费累计(USDT)
close_pnl = 0              -- 期末强平(CLOSE- fill)锁定损益(持仓盈亏落袋)
stall_bars = 0             -- 停摆可观测: 重挂需求持续未满足的 bar 数(挂出/成交即清零)
stall_bars_max = 0
stall_reported = false     -- WARN 去重: 每段停摆只报一次
max_lots = 0               -- 待配对栈深度峰值(网格层数峰值)

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

-- lots 序列化: "qty:price;qty:price;..." (无 json 库, 手动拼接)
local function serialize_lots()
    local parts = {}
    for i, lot in ipairs(lots) do
        parts[#parts + 1] = string.format("%.10f:%.10f", lot.qty, lot.price)
    end
    return table.concat(parts, ";")
end

local function deserialize_lots(s)
    lots = {}
    if s == nil or s == "" then
        return
    end
    for part in string.gmatch(s, "[^;]+") do
        local qty, price = part:match("([^:]+):([^:]+)")
        if qty and price then
            local q = tonumber(qty)
            local p = tonumber(price)
            if q and p and q > 0 and p > 0 then
                lots[#lots + 1] = { qty = q, price = p }
            end
        end
    end
end

-- 方向偏移不对称间距: 返回 (down_pct, up_pct) 百分比(小数, 0.01=1%)。
--   flag<0(净买, 价格下探) → 下方间距%放大; flag>0(净卖, 价格上冲) → 上方间距%放大。
local function side_spacing(spacing_pct, flag_, offset)
    if flag_ < 0 then
        return spacing_pct * (1 + math.abs(flag_) * offset), spacing_pct
    elseif flag_ > 0 then
        return spacing_pct, spacing_pct * (1 + math.abs(flag_) * offset)
    end
    return spacing_pct, spacing_pct
end

-- 开多量保证金兜底(合约差异 #2): 名义/杠杆 ≤ 可用余额(预扣手续费 + 滑点余量)。
local function cap_open(ctx, size, p, leverage, fee_bps)
    local cash = ctx:balance(quote_asset) or 0
    local max_sz = cash * leverage / (p * (1 + (fee_bps + 20) / 10000))
    if size > max_sz then
        return max_sz
    end
    return size
end

-- 平多量持仓兜底: cap 到引擎实际多头持仓即可。防超卖由引擎兜底(split_fill/close_position 的
-- min(size, side_size), backtest.rs), 策略**不自砍** —— 1e-9 比例折扣会留残仓,
-- hedge 逐仓钱包 sweep 永不触发 → 保证金滞留停摆(032 复审根因 A, 严禁恢复)。
local function cap_close(ctx, pair, size)
    local pos = ctx:pos_size(pair, "long") or 0
    if size > pos then
        return pos
    end
    return size
end

function save_state(ctx)
    if ref_price ~= nil then
        ctx:state_set("ref_price", string.format("%.10f", ref_price))
    end
    ctx:state_set("flag", string.format("%d", flag))
    ctx:state_set("lots", serialize_lots())
    ctx:state_set("init_lots", string.format("%d", init_lots))
    ctx:state_set("built", built and "1" or "0")
    -- 终态持久化(防重启复活, 同 shannon_spot_grid 审计教训)
    ctx:state_set("halted", halted and "1" or "0")
    -- 建仓单在途标记(防重启重复建仓)
    ctx:state_set("pending_entry", pending_entry and "1" or "0")
end

function on_init(ctx)
    local pair = ctx:config_str("pair")
    quote_asset = exec.detect_quote(pair)
    ctx:need_klines("primary", cfg_str(ctx, "interval", "1h"), 1000)

    built = false
    ref_price = nil
    flag = 0
    lots = {}
    init_lots = 0
    pending_entry = false
    need_rehang = false
    halted = false
    fatal = 0
    cost_checked = false
    last_bar_ts = nil
    liq_warned = false
    fill_count = 0
    buy_count = 0
    sell_count = 0
    skip_notional = 0
    rehang_count = 0
    track_up_count = 0
    coin_accumulated = 0
    quote_accumulated = 0
    fee_acc = 0
    close_pnl = 0
    stall_bars = 0
    stall_bars_max = 0
    stall_reported = false
    max_lots = 0
    flag_max = 0
    flag_min = 0

    -- 断点续接: 引擎已把上次会话的状态注入(表 strategy_state)
    local rp = ctx:state_get("ref_price")
    if rp ~= nil and tonumber(rp) and tonumber(rp) > 0 then
        ref_price = tonumber(rp)
        built = true
        flag = tonumber(ctx:state_get("flag")) or 0
        init_lots = tonumber(ctx:state_get("init_lots")) or 0
        deserialize_lots(ctx:state_get("lots"))
        -- 重启后必须重挂(停机撤单兜底已清掉本策略挂单)
        need_rehang = true
        pending_entry = (ctx:state_get("pending_entry") == "1")
        halted = (ctx:state_get("halted") == "1")
        if halted then
            ctx:log(string.format(
                "[paired_grid_futures_long] 续接: 上次已停机(halted=%s) -> 本次不再交易",
                tostring(halted)))
        end
        ctx:log(string.format(
            "[paired_grid_futures_long] 续接上次状态: 参考价 %.4f, flag=%d, 待配对仓 %d 笔",
            ref_price, flag, #lots))
    end

    ctx:log(string.format(
        "[paired_grid_futures_long] init pair=%s quote=%s 间距=%.2f%% 每格=%.2f 建仓=%.2f " ..
        "偏移=%.2f 模式=%s min_notional=%.2f fee_side=%.4f leverage=%.1f",
        pair, quote_asset, num(ctx, "spacing_pct", 0.01) * 100,
        num(ctx, "order_amount", 10), num(ctx, "initial_buy_amount", 0),
        num(ctx, "direction_offset", 0.2), cfg_str(ctx, "accumulate_mode", "u"),
        num(ctx, "min_notional", 50), num(ctx, "fee_side", 0.0005),
        num(ctx, "leverage", 2)))

    ctx:log(string.format(
        "[paired_grid_futures_long] 建仓: 价格低于 start_price=%.4f 才激活%s",
        num(ctx, "start_price", 0),
        num(ctx, "initial_buy_amount", 0) > 0
            and string.format(", 激活后市价买入 %.2f %s(拆成多笔 order_amount 方便配对)", num(ctx, "initial_buy_amount", 0), quote_asset)
            or ", 不建初始仓"))
end

function on_tick(ctx)
    if halted then
        return {}
    end
    local pair = ctx:config_str("pair")

    -- 决策节流: 只在主时钟新 bar 时决策一次(同现货, 防 O(n²) 全量克隆)。
    local t = ctx:now()
    if t == nil or t.ts == nil then
        return {}
    end
    if t.ts == last_bar_ts then
        return {}
    end

    local price = ctx:price(pair)
    last_price = price or last_price
    if not price or price <= 0 then
        return {}
    end
    -- 决策条件齐备 -> 现在才落节流标记(防取不到价格时白白消耗决策机会)
    last_bar_ts = t.ts

    local spacing_pct = num(ctx, "spacing_pct", 0.01)
    local order_amount = num(ctx, "order_amount", 10)
    local direction_offset = num(ctx, "direction_offset", 0.2)
    local accumulate_mode = cfg_str(ctx, "accumulate_mode", "u")
    local min_notional = num(ctx, "min_notional", 50)
    local fee_side = num(ctx, "fee_side", 0.0005)
    local fee_bps = fee_side * 10000
    local leverage = num(ctx, "leverage", 2)

    -- 配对保底最小价差: 卖价必须 ≥ 栈顶买入价 × (1+min_pair_profit)。默认 0 = 2×fee_side。
    local min_pair_profit = num(ctx, "min_pair_profit", 0)
    if min_pair_profit <= 0 then
        min_pair_profit = 2 * fee_side
    end

    -- 成本门槛(启动硬校验一次): spacing_pct 必须 > 2×fee_side, 否则每次配对往返净亏。
    if not cost_checked then
        cost_checked = true
        if spacing_pct <= 2 * fee_side then
            halted = true
            fatal = 1
            ctx:log(string.format(
                "[paired_grid_futures_long] [FATAL] 成本门槛不满足 -> 停机: spacing_pct=%.4f%% " ..
                "必须 > 2×fee_side=%.4f%%; 价格=%.4f fee_side=%.4f。",
                spacing_pct * 100, 2 * fee_side * 100, price, fee_side))
            return {}
        end
        ctx:log(string.format(
            "[paired_grid_futures_long] 成本门槛通过(首次校验): spacing_pct=%.4f%% > %.4f%%",
            spacing_pct * 100, 2 * fee_side * 100))
    end

    -- ── 未激活: 价格 < start_price 才激活(同现货)
    if not built then
        if pending_entry then
            return {} -- 建仓市价单已发出, 等成交回调
        end
        local start_px = num(ctx, "start_price", 0)
        if start_px <= 0 then
            halted = true
            fatal = 1
            ctx:log("[paired_grid_futures_long] [FATAL] 缺少必填参数 start_price(开始价格) -> 停机: " ..
                "策略只在价格低于 start_price 时激活。")
            return {}
        end
        if price >= start_px then
            return {}
        end
        -- 激活: 可选市价建仓(合约差异 #1: 带 position_side)
        local amt = num(ctx, "initial_buy_amount", 0)
        if amt > 0 then
            local size = cap_open(ctx, amt / price, price, leverage, fee_bps)
            if size > 0 and size * price >= min_notional then
                pending_entry = true
                ctx:log(string.format(
                    "[paired_grid_futures_long] 激活(价 %.4f < 开始价 %.4f) -> 市价买入建仓 %.6f (≈%.2f %s), " ..
                    "成交后拆成多个 order_amount 的配对仓(不计入 flag)",
                    price, start_px, size, size * price, quote_asset))
                return {
                    { pair = pair, side = "buy", size = size, order_type = "market", position_side = "long" },
                }
            end
            ctx:log(string.format(
                "[paired_grid_futures_long] 激活但建仓名义 %.2f < min_notional %.2f -> 按无建仓处理",
                size * price, min_notional))
        end
        built = true
        ref_price = price
        need_rehang = true
        save_state(ctx)
        ctx:log(string.format(
            "[paired_grid_futures_long] 激活(价 %.4f < 开始价 %.4f), 未建仓 -> 参考价 := 现价 %.4f, 进入网格",
            price, start_px, ref_price))
        return {}
    end

    -- ── 已激活
    if ref_price == nil then
        return {}
    end

    -- 追踪(同现货, 用户 2026-09-24 口径): 栈空且价格上涨超过一个间距 -> 参考价上移一个间距,
    -- 让下方买单跟随价格(否则买单永远成交不了, 网格脱节)。
    local down_pct, up_pct = side_spacing(spacing_pct, flag, direction_offset)
    while #lots == 0 and price >= ref_price * (1 + up_pct) and up_pct > 0 do
        ref_price = ref_price * (1 + up_pct)
        track_up_count = track_up_count + 1
        need_rehang = true
    end

    -- 爆仓价监控(合约差异 #3): 多头距爆仓价 < liq_warn_ratio 打 WARN, 只告警不动作(去重防刷屏)。
    local pos_now = ctx:pos_size(pair, "long") or 0
    if pos_now > 0 then
        local liq = ctx:pos_liq(pair, "long")
        if liq and liq > 0 then
            local dist = (price - liq) / price
            if dist < num(ctx, "liq_warn_ratio", 0.1) then
                if not liq_warned then
                    liq_warned = true
                    ctx:log(string.format(
                        "[paired_grid_futures_long] [WARN] 多头逼近爆仓价: 现价 %.4f 爆仓价 %.4f 距离 %.2f%% (< %.2f%%)",
                        price, liq, dist * 100, num(ctx, "liq_warn_ratio", 0.1) * 100))
                end
            elseif dist >= num(ctx, "liq_warn_ratio", 0.1) * 1.5 then
                liq_warned = false -- 距离回升: 重置, 下次再逼近再告警
            end
        end
    else
        liq_warned = false
    end

    if not need_rehang then
        return {}
    end

    -- 重挂(全撤重挂): 撤旧单 + 下方买单 + 上方卖单(栈非空才挂)。等比: 买 ÷(1+down)。
    down_pct, up_pct = side_spacing(spacing_pct, flag, direction_offset)
    local buy_px = ref_price / (1 + down_pct)
    -- 卖价锚定**栈顶买入成本**×(1+up)(2026-09-26 用户口径: 有仓位不随成交上移 —— 此前锚定
    -- ref_price(=最近成交价), 上涨段卖单逐笔爬升(82.15→82.97→83.80...), 相当于有仓还追踪上移;
    -- 现改只随 flag 扩大间距, 栈空后由追踪上移接管)。注意: 此为与现货的第 3 处语义差异
    -- (现货仍锚 ref_price)。
    local sell_px = nil
    if #lots > 0 then
        local top = lots[#lots]
        sell_px = top.price * (1 + up_pct)
        -- 配对保底: 卖价必须 ≥ 栈顶买入价 × (1+min_pair_profit)(防 V 型反转中买高卖低)。
        local min_sell = top.price * (1 + min_pair_profit)
        if sell_px < min_sell then
            sell_px = min_sell
        end
    end
    local orders = { { pair = pair, action = "cancel_pending" } }

    if buy_px > 0 then
        local buy_size = cap_open(ctx, order_amount / buy_px, buy_px, leverage, fee_bps)
        if buy_size > 0 and buy_size * buy_px >= min_notional then
            orders[#orders + 1] = {
                pair = pair, side = "buy", size = buy_size, order_type = "limit", price = buy_px,
                position_side = "long",
            }
        end
    end

    if #lots > 0 and sell_px > 0 then
        local top = lots[#lots]
        local sell_size
        if accumulate_mode == "coin" then
            sell_size = order_amount * (1 + 2 * fee_side) / sell_px
        else
            sell_size = top.qty
        end
        sell_size = cap_close(ctx, pair, sell_size)
        if sell_size > 0 and sell_size * sell_px >= min_notional then
            orders[#orders + 1] = {
                pair = pair, side = "sell", size = sell_size, order_type = "limit", price = sell_px,
                position_side = "long",
            }
        end
    end

    -- 两手都没挂出(量太小/名义不足): 保持 need_rehang, 下一 tick 再试(防永久停摆) + 停摆可观测
    if #orders <= 1 then
        skip_notional = skip_notional + 1
        stall_bars = stall_bars + 1
        if stall_bars > stall_bars_max then
            stall_bars_max = stall_bars
        end
        if not stall_reported and stall_bars >= 1440 then
            stall_reported = true
            ctx:log(string.format(
                "[paired_grid_futures_long] [WARN] 连续 %d 根 bar 挂单失败(小名义/资金不足), cash=%.2f",
                stall_bars, ctx:balance(quote_asset) or 0))
        end
        return {}
    end

    stall_bars = 0
    need_rehang = false
    rehang_count = rehang_count + 1
    save_state(ctx)
    ctx:log(string.format(
        "[paired_grid_futures_long] 网格重挂 #%d: 参考价 %.4f 间距=%.2f%% 下间距=%.2f%% 上间距=%.2f%% flag=%d " ..
        "待配对 %d 笔 -> 买 %.4f / 卖 %s",
        rehang_count, ref_price, spacing_pct * 100, down_pct * 100, up_pct * 100, flag, #lots,
        buy_px, #lots > 0 and string.format("%.4f", sell_px) or "无(无待配对仓)"))
    return orders
end

function on_fill(ctx, fill)
    -- 只做多: 非 long 侧成交忽略(防御, 正常不应出现)
    if fill.position_side and fill.position_side ~= "long" then
        return
    end
    local px = fill.fill_price or ref_price
    local size = fill.fill_size or 0
    local pair = ctx:config_str("pair")
    fee_acc = fee_acc + (fill.fee or 0)

    if pending_entry then
        -- 建仓成交: 拆成多个 order_amount 的配对仓, **不计入 flag**(同现货); 计入成交/手续费统计。
        -- 2026-09-26 用户口径: 建仓 lot 记 init 标记, 之后被网格卖出时也**不 flag+1**(见 sell 分支)。
        pending_entry = false
        built = true
        ref_price = px
        fill_count = fill_count + 1
        buy_count = buy_count + 1
        local amt = num(ctx, "initial_buy_amount", 0)
        local order_amount = num(ctx, "order_amount", 10)
        local n = math.max(1, math.floor(amt / order_amount + 0.5))
        local qty_per = size / n
        for _ = 1, n do
            lots[#lots + 1] = { qty = qty_per, price = px }
        end
        init_lots = n
        need_rehang = true
        stall_bars = 0
        save_state(ctx)
        ctx:log(string.format(
            "[paired_grid_futures_long] 建仓成交 %.6f @ %.4f -> 拆成 %d 笔配对仓(每笔约 %.2f %s), 参考价 := %.4f, flag 不变=%d",
            size, px, n, qty_per * px, quote_asset, ref_price, flag))
        return
    end

    -- 网格单成交: 更新 lot / flag / ref_price, 全撤重挂(同现货)
    ref_price = px
    -- flag 只统计**网格自建** lot 的平仓: 卖掉建仓(init) lot 不 flag+1,
    -- 否则建仓越厚 flag 冲得越高、卖单间距被放大到追不上行情(2026-09-26 用户口径)。
    local grid_lots_sold = 0
    if fill.side == "buy" then
        lots[#lots + 1] = { qty = size, price = px }
        flag = flag - 1
    else
        -- 期末强平单(引擎 force_close_all, "CLOSE-" 前缀)可能跨多 lot: u 模式按 fill size
        -- 逐层扣减(正常卖单 size==栈顶, 与现货整 lot 出栈等价); 强平损益单列 close_pnl。
        -- 1e-9 仅作 f64 账本簿记尘阈值(引擎 Decimal 精确, f64 转换末位差 1ulp), 不触碰资金量。
        local is_close_fill = (fill.client_order_id or ""):sub(1, 6) == "CLOSE-"
        if #lots > 0 and not is_close_fill and cfg_str(ctx, "accumulate_mode", "u") == "coin" then
            -- coin 模式(同现货): 卖出只覆盖本金, 价差以"币"形式留存; 整 lot 出栈。
            coin_accumulated = coin_accumulated + math.max(0, lots[#lots].qty - size)
            if #lots <= init_lots then
                init_lots = init_lots - 1
            else
                grid_lots_sold = 1
            end
            table.remove(lots)
        elseif #lots > 0 then
            local remain = size
            while remain > 1e-9 and #lots > 0 do
                local top = lots[#lots]
                local matched = math.min(remain, top.qty)
                local g = matched * (px - top.price)
                if is_close_fill then
                    close_pnl = close_pnl + g
                else
                    quote_accumulated = quote_accumulated + g
                end
                remain = remain - matched
                top.qty = top.qty - matched
                if top.qty < 1e-9 then
                    top.qty = 0
                end
                if top.qty == 0 then
                    -- 建仓 lot 恒在栈底(建仓最先、网格买在其上): 索引 <= init_lots 即 init。
                    if #lots <= init_lots then
                        init_lots = init_lots - 1
                    else
                        grid_lots_sold = grid_lots_sold + 1
                    end
                    table.remove(lots)
                end
            end
        end
        flag = flag + grid_lots_sold
    end
    if flag > flag_max then
        flag_max = flag
    end
    if flag < flag_min then
        flag_min = flag
    end
    if #lots > max_lots then
        max_lots = #lots
    end

    if fill.side == "buy" then
        buy_count = buy_count + 1
    else
        sell_count = sell_count + 1
    end
    fill_count = fill_count + 1
    need_rehang = true
    stall_bars = 0
    stall_reported = false

    local pos_now = ctx:pos_size(pair, "long") or 0

    save_state(ctx)
    ctx:log(string.format(
        "[FILL] #%d %s size=%.6f px=%.4f notional=%.2f | flag=%d 待配对=%d 参考价(新)=%.4f 持仓=%.6f 权益=%.2f",
        fill_count, fill.side, size, px, size * px, flag, #lots, ref_price, pos_now, ctx:equity() or 0))
end

-- 拒单回传: 重挂防停摆(同现货)。cancelled 是主动撤单的正常回传, 不触发重挂(防死循环)。
function on_order_update(ctx, upd)
    local status = upd and upd.status
    if status == "rejected" then
        if pending_entry then
            pending_entry = false -- 建仓被拒: 恢复, 下个 tick 重新走激活
        end
        need_rehang = true
        ctx:log(string.format(
            "[paired_grid_futures_long] 挂单被拒: pair=%s filled=%.6f remaining=%.6f → 重挂",
            upd.pair, upd.filled_size or 0, upd.remaining_size or 0))
    end
end

function on_stop(ctx)
    local pair = ctx:config_str("pair")
    local pos = ctx:pos_size(pair, "long") or 0
    -- 分层归因统计(经 strategy_stats 透传到回测报告"策略统计"区块)
    ctx:state_set("stat_eff_spacing_pct", string.format("%.6f", num(ctx, "spacing_pct", 0.01)))
    ctx:state_set("stat_eff_direction_offset", string.format("%.6f", num(ctx, "direction_offset", 0.2)))
    ctx:state_set("stat_eff_min_pair_profit", string.format("%.6f",
        (function()
            local mpp = num(ctx, "min_pair_profit", 0)
            if mpp <= 0 then
                mpp = 2 * num(ctx, "fee_side", 0.0005)
            end
            return mpp
        end)()))
    ctx:state_set("stat_fill_count", string.format("%d", fill_count))
    ctx:state_set("stat_buy_count", string.format("%d", buy_count))
    ctx:state_set("stat_sell_count", string.format("%d", sell_count))
    ctx:state_set("stat_fee", string.format("%.4f", fee_acc))
    ctx:state_set("stat_quote_accumulated", string.format("%.6f", quote_accumulated))
    ctx:state_set("stat_coin_accumulated", string.format("%.6f", coin_accumulated))
    ctx:state_set("stat_close_pnl", string.format("%.6f", close_pnl))
    ctx:state_set("stat_track_up_count", string.format("%d", track_up_count))
    ctx:state_set("stat_rehang_count", string.format("%d", rehang_count))
    ctx:state_set("stat_skip_notional", string.format("%d", skip_notional))
    ctx:state_set("stat_flag_max", string.format("%d", flag_max))
    ctx:state_set("stat_flag_min", string.format("%d", flag_min))
    ctx:state_set("stat_init_lots", string.format("%d", init_lots))
    ctx:state_set("stat_max_lots", string.format("%d", max_lots))
    ctx:state_set("stat_stall_bars", string.format("%d", stall_bars_max))
    ctx:state_set("stat_cash_final", string.format("%.4f", ctx:balance(quote_asset) or 0))
    ctx:log(string.format(
        "[paired_grid_futures_long] 停机(**不清仓**): 成交 %d(买 %d 卖 %d) / 跳过(小名义 %d) / " ..
        "追踪上移 %d / flag=%d(峰值 +%d/%d) 待配对 %d 参考价 %s / %s / 期末持仓 %.6f 权益 %.2f / " ..
        "积累币 %.6f / 积累U %.2f / 强平锁定 %.2f / 手续费 %.4f",
        fill_count, buy_count, sell_count, skip_notional, track_up_count,
        flag, flag_max, flag_min, #lots, ref_price and string.format("%.4f", ref_price) or "nil",
        (halted and "已停机(成本门槛)" or "正常运行"),
        pos, ctx:equity() or 0, coin_accumulated, quote_accumulated, close_pnl, fee_acc))
end
