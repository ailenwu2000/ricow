-- 现货动态非对称网格 (paired_grid) -- v1 (2026-09-24)
--
-- ═══ 一句话 ═══
--   固定金额(order_amount)的现货动态非对称网格: 价格低于 start_price 激活, 以最近成交价为参考价上下各挂一单
--   (下方固定金额买单、上方配对卖单), 配对卖价恒 > 买价; 方向标志(买 −1 / 卖 +1)驱动上下间距不对称放大,
--   抑制单向成交、让 flag 回归 0。
--
-- ═══ 策略含义(用户 2026-09-24 口径, 逐条) ═══
--   1) 激活: 必须给 start_price(开始价格); 价格**低于**它才激活。
--   2) 建仓(可选): 激活后若 initial_buy_amount > 0, 市价买入该金额, 成交后按 order_amount 拆成
--      多个 lot(方便配对)。**建仓不计入 flag**(flag 从建仓后的网格交易才起算)。
--   3) 网格: 参考价 = 最近成交价(ref_price); 下方挂 1 张买单(ref ÷ (1+下方间距%), 金额 order_amount),
--      上方挂 1 张卖单(ref × (1+上方间距%), 数量 = 栈顶 lot 的币数), 仅当有待配对仓(lots 非空)才挂卖单。
--   4) 间距(等比): 固定百分比 spacing_pct(默认 1%), 上下用等比公比对称(向上 ×(1+s), 向下 ÷(1+s))。
--      买价 = ref ÷ (1 + 下方间距%), 卖价 = ref × (1 + 上方间距%)
--      下方间距% = spacing_pct × (flag<0 ? 1+|flag|×direction_offset : 1)
--      上方间距% = spacing_pct × (flag>0 ? 1+|flag|×direction_offset : 1)
--   5) 方向标志 flag: 买 −1 / 卖 +1, 简单累计, 无上限, 永不手动归零。初始 0。
--   6) 配对 = LIFO(栈顶 = 最近买入 = 价格最接近当前价): 保证"卖价 > 买价 × (1+min_pair_profit)"恒成立
--      (min_pair_profit 默认 0.2% = 往返手续费, 不必严格 = spacing_pct, 趋势市场快速还原仓位)。
--   7) 成交(全撤重挂): 买单成交 → lot 入栈 + flag−1; 卖单成交 → 栈顶出栈 + flag+1;
--      两者都 ref_price = 成交价, 撤光重挂。
--      成交模式 accumulate_mode:
--        "u"(默认):    卖出 = 栈顶 lot 全部币(积累计价币 U, 如 USDT/USDC/FDUSD)。
--        "coin":        卖出 = order_amount×(1+2×fee_side)/卖价(覆盖买卖两笔手续费), 多出的币积累不再配对。
--   8) 追踪: 栈空(无待配对仓)且价格 ≥ ref_price + 上方间距 → ref_price 上移一个间距重挂, 让下方买单跟随上涨价格。
--   9) 成本门槛: spacing_pct 必须 > 2×fee_side(0.2%, 配对往返 = 买+卖各 0.1%), 不满足 [FATAL] 停机。
--  10) 结束: 无持仓且无待配对仓 → 结束(不再动作); 停机**不清仓**。
--
-- ═══ 参数(全部可配, 默认值可直接回测) ═══
--   pair              必填, 交易对(现货)
--   start_price       必填, 开始价格(低于它才激活)
--   interval          主时钟, 默认 1h
--   spacing_pct       间距百分比(小数, 0.01=1%), 默认 0.01
--   order_amount      每笔买入资金(计价币, 如 U), 默认 10
--   initial_buy_amount  初始建仓金额, 默认 0(=不建仓)
--   direction_offset  方向偏移, 默认 0.2
--   accumulate_mode   成交模式, 默认 "u"(积累计价币 U); "coin" = 积累币
--   min_notional      单笔最小名义, 默认 5
--   fee_side          单边费率, 默认 0.001(现货 0.1%/边)
--
-- ═══ 用法 ═══
--   ricow backtest --strategy paired_grid --pair SOLUSDT --interval 1h --days 90 --cash 1000 \
--     --param start_price=<现价> --param order_amount=10 --param initial_buy_amount=30
--   TOML: type = "paired_grid" + [strategy.params] 填上面参数表。
--
quote_asset = ""  -- 计价资产, on_init 里 detect_quote(pair) 动态检测(USDT/USDC/FDUSD 等)
-- 状态
built = false
ref_price = nil
flag = 0
flag_max = 0               -- flag 历史最大值(最多卖出几个网格)
flag_min = 0               -- flag 历史最小值(负数, 最大买入几个网格)
lots = {}                  -- LIFO 栈: 待配对买入仓, 每项 {qty=, price=}
pending_entry = false      -- 建仓市价单在途
need_rehang = false        -- 成交/追踪后置 true: 下一 tick 撤旧单 + 重挂
finished = false           -- 无持仓且无待配对仓 → 结束
halted = false             -- 成本门槛不满足 → 停机
cost_checked = false       -- 成本门槛只做一次启动校验
last_bar_ts = nil
-- 计数(可观测性)
fill_count = 0
buy_count = 0
sell_count = 0
skip_notional = 0
rehang_count = 0
track_up_count = 0
last_price = 0
coin_accumulated = 0       -- coin 模式配对积累的币(每笔卖出时, 栈顶 lot 未卖完的余额累积, 不再配对)
quote_accumulated = 0       -- u 模式配对积累的计价币(每笔卖出回笼 − 配对买入本金 = 价差, 毛额未扣手续费)

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

-- 买量现金兜底(预扣手续费 + 滑点余量, 同 shannon_spot_grid 口径)
local function cap_buy(ctx, size, p, fee_bps)
    local cash = ctx:balance(quote_asset) or 0
    local max_buy = cash / (p * (1 + (fee_bps + 20) / 10000))
    if size > max_buy then
        return max_buy
    end
    return size
end

-- 卖量持仓兜底(留 1e-9 精度折扣, 防 f64 往返超卖)
local function cap_sell(ctx, pair, size)
    local pos = (ctx:position_size(pair) or 0) * (1 - 1e-9)
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
    ctx:state_set("built", built and "1" or "0")
    -- 终态持久化(防重启复活, 同 shannon_spot_grid 审计教训)
    ctx:state_set("halted", halted and "1" or "0")
    ctx:state_set("finished", finished and "1" or "0")
    -- 建仓单在途标记(防重启重复建仓)
    ctx:state_set("pending_entry", pending_entry and "1" or "0")
end

function on_init(ctx)
    local pair = ctx:config_str("pair")
    quote_asset = exec.detect_quote(pair)
    -- 数据需求声明(策略 → 引擎): 主时钟。周期与根数由策略显式给出。
    ctx:need_klines("primary", cfg_str(ctx, "interval", "1h"), 1000)

    built = false
    ref_price = nil
    flag = 0
    lots = {}
    pending_entry = false
    need_rehang = false
    halted = false
    finished = false
    cost_checked = false
    last_bar_ts = nil

    -- 断点续接: 引擎已把上次会话的状态注入(表 strategy_state)
    local rp = ctx:state_get("ref_price")
    if rp ~= nil and tonumber(rp) and tonumber(rp) > 0 then
        ref_price = tonumber(rp)
        built = true
        flag = tonumber(ctx:state_get("flag")) or 0
        deserialize_lots(ctx:state_get("lots"))
        -- 重启后必须重挂(停机撤单兜底已清掉本策略挂单)
        need_rehang = true
        pending_entry = (ctx:state_get("pending_entry") == "1")
        halted = (ctx:state_get("halted") == "1")
        finished = (ctx:state_get("finished") == "1")
        if halted or finished then
            ctx:log(string.format(
                "[paired_grid] 续接: 上次已停机(halted=%s, finished=%s) -> 本次不再交易",
                tostring(halted), tostring(finished)))
        end
        ctx:log(string.format(
            "[paired_grid] 续接上次状态: 参考价 %.4f, flag=%d, 待配对仓 %d 笔",
            ref_price, flag, #lots))
    end

    ctx:log(string.format(
        "[paired_grid] init pair=%s quote=%s 间距=%.2f%% 每笔=%.2f 建仓=%.2f " ..
        "偏移=%.2f 模式=%s min_notional=%.2f fee_side=%.4f",
        pair, quote_asset, num(ctx, "spacing_pct", 0.01) * 100,
        num(ctx, "order_amount", 10), num(ctx, "initial_buy_amount", 0),
        num(ctx, "direction_offset", 0.2), cfg_str(ctx, "accumulate_mode", "u"),
        num(ctx, "min_notional", 5), num(ctx, "fee_side", 0.001)))

    ctx:log(string.format(
        "[paired_grid] 建仓: 价格低于 start_price=%.4f 才激活%s",
        num(ctx, "start_price", 0),
        num(ctx, "initial_buy_amount", 0) > 0
            and string.format(", 激活后市价买入 %.2f %s(拆成多笔 order_amount 方便配对)", num(ctx, "initial_buy_amount", 0), quote_asset)
            or ", 不建初始仓"))
end

function on_tick(ctx)
    if finished or halted then
        return {}
    end
    local pair = ctx:config_str("pair")

    -- 决策节流: 只在主时钟新 bar 时决策一次。
    -- 用 ctx:now() 取当前 bar 的 open_time 做节流, 而非 ctx:klines —— 单标的路径下 klines 返回
    -- **全部**已收盘序列(030 撤销尾窗截断), 每 tick 全量克隆是 O(n²), 1m×60 天 = 86400 根会卡死。
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
    local min_notional = num(ctx, "min_notional", 5)
    local fee_side = num(ctx, "fee_side", 0.001)
    local fee_bps = fee_side * 10000

    -- 配对保底最小价差: 卖价必须 ≥ 栈顶买入价 × (1+min_pair_profit)。
    -- 默认 0 = 用 2×fee_side(往返手续费 0.2%), 不必严格 = spacing_pct(1%)——
    -- 趋势市场中放宽到最小间隔, 让卖出更快成交、快速还原仓位(flag 归零)。
    local min_pair_profit = num(ctx, "min_pair_profit", 0)
    if min_pair_profit <= 0 then
        min_pair_profit = 2 * fee_side
    end

    -- 成本门槛(启动硬校验一次): spacing_pct 必须 > 2×fee_side(0.2%), 否则每次配对往返净亏。
    -- 依据: 等比配对价差 = 卖价/买价 − 1 ≈ spacing_pct, 往返成本 = 买+卖各 0.1%。
    if not cost_checked then
        cost_checked = true
        if spacing_pct <= 2 * fee_side then
            halted = true
            ctx:log(string.format(
                "[paired_grid] [FATAL] 成本门槛不满足 -> 停机: spacing_pct=%.4f%% " ..
                "必须 > 2×fee_side=%.4f%%; 价格=%.4f fee_side=%.4f。",
                spacing_pct * 100, 2 * fee_side * 100, price, fee_side))
            return {}
        end
        ctx:log(string.format(
            "[paired_grid] 成本门槛通过(首次校验): spacing_pct=%.4f%% > %.4f%%",
            spacing_pct * 100, 2 * fee_side * 100))
    end

    -- ── 未激活: 价格 < start_price 才激活
    if not built then
        if pending_entry then
            return {} -- 建仓市价单已发出, 等成交回调
        end
        local start_px = num(ctx, "start_price", 0)
        if start_px <= 0 then
            halted = true
            ctx:log("[paired_grid] [FATAL] 缺少必填参数 start_price(开始价格) -> 停机: " ..
                "策略只在价格低于 start_price 时激活。")
            return {}
        end
        if price >= start_px then
            return {}
        end
        -- 激活: 可选市价建仓
        local amt = num(ctx, "initial_buy_amount", 0)
        if amt > 0 then
            local size = cap_buy(ctx, amt / price, price, fee_bps)
            if size > 0 and size * price >= min_notional then
                pending_entry = true
                ctx:log(string.format(
                    "[paired_grid] 激活(价 %.4f < 开始价 %.4f) -> 市价买入建仓 %.6f (≈%.2f %s), " ..
                    "成交后拆成多个 order_amount 的配对仓(不计入 flag)",
                    price, start_px, size, size * price, quote_asset))
                return {
                    { pair = pair, side = "buy", size = size, order_type = "market" },
                }
            end
            ctx:log(string.format(
                "[paired_grid] 激活但建仓名义 %.2f < min_notional %.2f -> 按无建仓处理",
                size * price, min_notional))
        end
        built = true
        ref_price = price
        need_rehang = true
        save_state(ctx)
        ctx:log(string.format(
            "[paired_grid] 激活(价 %.4f < 开始价 %.4f), 未建仓 -> 参考价 := 现价 %.4f, 进入网格",
            price, start_px, ref_price))
        return {}
    end

    -- ── 已激活
    if ref_price == nil then
        return {}
    end

    -- 追踪(用户 2026-09-24 第 4 点补充): 栈空(无待配对仓)且价格上涨超过一个间距 ->
    -- 参考价上移一个间距(等比 ×(1+up_pct)), 让下方买单跟随价格(否则买单永远成交不了, 网格脱节)。
    local down_pct, up_pct = side_spacing(spacing_pct, flag, direction_offset)
    while #lots == 0 and price >= ref_price * (1 + up_pct) and up_pct > 0 do
        ref_price = ref_price * (1 + up_pct)
        track_up_count = track_up_count + 1
        need_rehang = true
    end

    if not need_rehang then
        return {}
    end

    -- 重挂(全撤重挂): 撤旧单 + 下方买单 + 上方卖单(栈非空才挂)。等比: 买 ÷(1+down), 卖 ×(1+up)。
    down_pct, up_pct = side_spacing(spacing_pct, flag, direction_offset)
    local buy_px = ref_price / (1 + down_pct)
    local sell_px = ref_price * (1 + up_pct)
    -- 配对保底: 卖价必须 ≥ 栈顶买入价 × (1+min_pair_profit)。
    -- 否则 V 型反转(先跌后涨)中, ref_price 从低位回弹、而栈顶是下跌中高位买入的 lot,
    -- 卖价会被 ref_price 拉到买入价以下 → 买高卖低的亏损配对(违反"卖价>买价+最小间隔")。
    if #lots > 0 then
        local min_sell = lots[#lots].price * (1 + min_pair_profit)
        if sell_px < min_sell then
            sell_px = min_sell
        end
    end
    local orders = { { pair = pair, action = "cancel_pending" } }

    if buy_px > 0 then
        local buy_size = cap_buy(ctx, order_amount / buy_px, buy_px, fee_bps)
        if buy_size > 0 and buy_size * buy_px >= min_notional then
            orders[#orders + 1] = {
                pair = pair, side = "buy", size = buy_size, order_type = "limit", price = buy_px,
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
        sell_size = cap_sell(ctx, pair, sell_size)
        if sell_size > 0 and sell_size * sell_px >= min_notional then
            orders[#orders + 1] = {
                pair = pair, side = "sell", size = sell_size, order_type = "limit", price = sell_px,
            }
        end
    end

    -- 两手都没挂出(量太小/名义不足): 保持 need_rehang, 下一 tick 再试(防永久停摆)
    if #orders <= 1 then
        skip_notional = skip_notional + 1
        return {}
    end

    need_rehang = false
    rehang_count = rehang_count + 1
    save_state(ctx)
    ctx:log(string.format(
        "[paired_grid] 网格重挂 #%d: 参考价 %.4f 间距=%.2f%% 下间距=%.2f%% 上间距=%.2f%% flag=%d " ..
        "待配对 %d 笔 -> 买 %.4f / 卖 %s",
        rehang_count, ref_price, spacing_pct * 100, down_pct * 100, up_pct * 100, flag, #lots,
        buy_px, #lots > 0 and string.format("%.4f", sell_px) or "无(无待配对仓)"))
    return orders
end

function on_fill(ctx, fill)
    local px = fill.fill_price or ref_price
    local size = fill.fill_size or 0
    local pair = ctx:config_str("pair")

    if pending_entry then
        -- 建仓成交: 拆成多个 order_amount 的配对仓, **不计入 flag**
        pending_entry = false
        built = true
        ref_price = px
        local amt = num(ctx, "initial_buy_amount", 0)
        local order_amount = num(ctx, "order_amount", 10)
        local n = math.max(1, math.floor(amt / order_amount + 0.5))
        local qty_per = size / n
        for _ = 1, n do
            lots[#lots + 1] = { qty = qty_per, price = px }
        end
        need_rehang = true
        save_state(ctx)
        ctx:log(string.format(
            "[paired_grid] 建仓成交 %.6f @ %.4f -> 拆成 %d 笔配对仓(每笔约 %.2f %s), 参考价 := %.4f, flag 不变=%d",
            size, px, n, qty_per * px, quote_asset, ref_price, flag))
        return
    end

    -- 网格单成交: 更新 lot / flag / ref_price, 全撤重挂
    ref_price = px
    if fill.side == "buy" then
        lots[#lots + 1] = { qty = size, price = px }
        flag = flag - 1
    else
        if #lots > 0 then
            -- 利润留存形式由 accumulate_mode 决定(二者互斥):
            --   coin: 卖出只覆盖本金, 价差全部以"币"形式留存(多出的币积累, 不再配对);
            --   u: 卖出 = 栈顶全部币, 价差全部以"U(计价币)"形式落袋。
            if cfg_str(ctx, "accumulate_mode", "u") == "coin" then
                coin_accumulated = coin_accumulated + math.max(0, lots[#lots].qty - size)
            else
                quote_accumulated = quote_accumulated + size * (px - lots[#lots].price)
            end
            table.remove(lots) -- 栈顶出栈(LIFO 配对)
        end
        flag = flag + 1
    end
    if flag > flag_max then
        flag_max = flag
    end
    if flag < flag_min then
        flag_min = flag
    end

    if fill.side == "buy" then
        buy_count = buy_count + 1
    else
        sell_count = sell_count + 1
    end
    fill_count = fill_count + 1
    need_rehang = true

    -- 无持仓且无待配对仓 -> 结束
    local pos_now = ctx:position_size(pair) or 0
    if pos_now <= 0 and #lots == 0 then
        finished = true
        ctx:log("[paired_grid] 无持仓且无待配对仓 → 结束策略(不再交易)")
    end

    save_state(ctx)
    ctx:log(string.format(
        "[FILL] #%d %s size=%.6f px=%.4f notional=%.2f | flag=%d 待配对=%d 参考价(新)=%.4f 持仓=%.6f 权益=%.2f",
        fill_count, fill.side, size, px, size * px, flag, #lots, ref_price, pos_now, ctx:equity() or 0))
end

-- 拒单回传: 重挂防停摆。
-- 只处理 rejected(挂单被拒, 真正"没成"): cancelled 是策略主动撤单(cancel_pending)的正常回传,
-- 重挂已在 on_tick 里处理; 若把 cancelled 也设 need_rehang, 会形成"重挂→撤单→回调→再重挂"死循环。
function on_order_update(ctx, upd)
    local status = upd and upd.status
    if status == "rejected" then
        if pending_entry then
            pending_entry = false -- 建仓被拒: 恢复, 下个 tick 重新走激活
        end
        need_rehang = true
        ctx:log(string.format(
            "[paired_grid] 挂单被拒: pair=%s filled=%.6f remaining=%.6f → 重挂",
            upd.pair, upd.filled_size or 0, upd.remaining_size or 0))
    end
end

function on_stop(ctx)
    local pair = ctx:config_str("pair")
    local pos = ctx:position_size(pair) or 0
    ctx:log(string.format(
        "[paired_grid] 停机(**不清仓**): 成交 %d(买 %d 卖 %d) / 跳过(小名义 %d) / " ..
        "追踪上移 %d / flag=%d(峰值 +%d/%d) 待配对 %d 参考价 %s / %s / 期末持仓 %.6f 权益 %.2f / 积累币 %.6f / 积累U %.2f",
        fill_count, buy_count, sell_count, skip_notional, track_up_count,
        flag, flag_max, flag_min, #lots, ref_price and string.format("%.4f", ref_price) or "nil",
        finished and "已结束" or (halted and "已停机(成本门槛)" or "正常运行"),
        pos, ctx:equity() or 0, coin_accumulated, quote_accumulated))
end
