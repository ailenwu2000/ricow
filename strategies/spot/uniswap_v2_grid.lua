-- 现货 Uniswap V2 网格 (uniswap_v2_grid) -- v1 (2026-09-26, 033)
--
-- ═══ 一句话 ═══
--   模拟 uniswap v2 池: 真实现金 C 与真实持仓 Q 价值恒 1:1; 以平衡价 ± atr_mult×ATR 挂买卖单,
--   任一成交都把成交价当作新平衡价、按"成交后 1:1 恢复"的量全撤重挂两侧 —— 离散化的恒定乘积再平衡。
--
-- ═══ 策略含义(用户 2026-09-26 口径, 逐条) ═══
--   1) 激活建仓: 给 start_price(开始价格); 价格**低于**它才激活。激活后市价买入 invest_cash 的一半
--      建仓( invest_cash=0 时用激活时 quote 余额的一半 ); 成交价 = 第一平衡价格。记录资金与仓位。
--   2) 挂单: 平衡价 + atr_mult×ATR 挂卖单, 平衡价 − atr_mult×ATR 挂买单(ATR 逐次重挂时取当前值
--      → 动态间距); 无追踪上移, 单子挂着等成交。
--   3) 回平衡量(成交后资金与仓位价值恢复 1:1, 含手续费修正):
--        买 q = (C − Q·p_b) / (p_b·(2+f))   [花 q·p_b·(1+f) 后 C′ = (Q+q)·p_b]
--        卖 q = (Q·p_s − C) / (p_s·(2−f))   [收 q·p_s·(1−f) 后 C′ = (Q−q)·p_s]
--      f = fee_side。C/Q 为**真实**余额与持仓(与 shannon 的虚拟账本不同, 无杠杆无目标权重)。
--   4) 成交: 平衡价 := 该笔成交价; 虚拟仓位不存在 —— 直接按真实成交更新策略账本模型; 全撤重挂两侧。
--   5) 结束语义: **无**。回平衡卖量 ≤ 持仓一半, 仓位永不清零、资金永不耗尽(公式自平衡), 全程运行,
--      与 uniswap 池语义一致。停机**不清仓**。
--
-- ═══ 与 shannon_spot_grid / paired_grid 的显式语义差异 ═══
--   - 账本 = 真实 C + 真实 Q(无 v_coin/v_cash 虚拟账本、无 leverage_mult、无 target_ratio、无趋势门控)。
--   - 挂单量公式目标是"成交后恰好 1:1"(含费修正), 不是固定金额、也不是 LIFO 配对栈。
--   - 无 flag、无追踪上移: 平衡价只在成交时移动(每次移动一个 ATR 间距的等价物)。
--
-- ═══ 参数(全部可配, 默认值可直接回测; 清单 [[params]] 同步声明 default 注入生效配置) ═══
--   pair              必填, 交易对(现货)
--   start_price       必填, 开始价格(低于它才激活)
--   invest_cash       投入资金, 默认 0(= 激活时 quote 余额全额); 激活时用其一半市价建仓
--   interval          主时钟, 默认 1h(回测建议 1m 数据提升重挂/撮合粒度)
--   atr_interval      算 ATR 的 K 线周期, 默认 1h
--   atr_period        ATR 周期, 默认 14
--   atr_mult          间距 = atr_mult × ATR, 默认 1
--   min_spacing_pct   最小间距(占价格比例), 默认 0.004(= 0.4%); 生效间距 = max(atr_mult×ATR, 该下限×价格),
--                     防止 ATR 过窄时网格被手续费磨损
--   min_notional      单笔最小名义, 默认 5
--   fee_side          单边费率, 默认 0.001(现货 0.1%/边)
--
-- ═══ 守卫与可观测性 ═══
--   ATR 未就绪 → 不挂单(计数); 买量受真实现金兜底(预扣费+滑点余量); 卖量受真实持仓兜底(1e-9 折扣);
--   buy_px ≤ 0 → 跳过买单(计数); 成本门槛: 生效间距/价格 < 4×fee_side(严格小于) → [FATAL] 停机
--   (0.4% 默认下限恰 = 4×0.1% 费率门槛, 一个往返毛利 = 间距 − 2×费率 = 0.2% > 0, 应放行);
--   连续 1 天无任何挂单 → WARN(停摆可观测性)。策略账本模型(m_cash/m_pos, 由 fill 独立记账含费)
--   在停机时与引擎余额/持仓交叉核对(误差 > 0.01 → WARN + stat_ledger_diff_* 留档)。
--
-- ═══ 用法 ═══
--   ricow backtest --strategy uniswap_v2_grid --pair SOLUSDT --interval 1m --days 180 --cash 10000 \
--     --param start_price=<窗口起点价> --param invest_cash=10000 --param atr_interval=1h
--   TOML: type = "uniswap_v2_grid" + [strategy.params] 填上面参数表。
--
quote_asset = ""  -- 计价资产, on_init 里 detect_quote(pair) 动态检测
-- 状态
built = false
balance_price = nil       -- 平衡价格(建仓成交价 / 最近一次网格成交价)
pending_entry = false     -- 建仓市价单在途
need_rehang = false       -- 成交/续接后置 true: 下一 tick 撤旧单 + 重挂两侧
halted = false            -- 成本门槛不满足 → 停机
cost_checked = false      -- 成本门槛只做一次启动校验
last_bar_ts = nil
invested0 = 0             -- 投入资金(激活时刻定格; 收益分解基准)
entry_price = 0           -- 初始建仓成交价(持仓损益基准)
entry_size = 0            -- 初始建仓数量
-- 策略账本模型(独立记账, 与引擎对账): 首笔成交时与引擎同步一次, 之后只按 fill 推进
m_cash = nil
m_pos = nil
m_fee = 0                 -- 模型累计手续费(Σ fill.fee)
last_price = 0
-- 停摆可观测性
stall_bars = 0            -- 处于"无任何挂单"状态的主时钟 bar 数(累计)
stall_since = nil         -- 进入停摆的 bar 时间戳 ms(按自然日 WARN)
stall_warned_at = nil     -- 上次 WARN 的时间戳 ms
-- 计数
fill_count = 0
buy_count = 0
sell_count = 0
skip_no_atr = 0
skip_notional = 0
skip_zero_buy_px = 0
rehang_count = 0

local function num(ctx, key, dflt)
    local v = ctx:config_f64(key)
    if v == nil or v == 0 then
        return dflt
    end
    return v
end

-- 买量现金兜底(预扣手续费 + 滑点余量, 同 shannon/paired 口径)
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

-- 模型账本按真实成交推进(fill.fee 已含, 引擎同源口径)
local function model_apply(fill, px, size)
    if m_cash == nil then
        return -- 首笔(建仓)成交在 on_fill 里同步初始值, 不会走到这里
    end
    if fill.side == "buy" then
        m_cash = m_cash - size * px - (fill.fee or 0)
        m_pos = m_pos + size
    else
        m_cash = m_cash + size * px - (fill.fee or 0)
        m_pos = m_pos - size
    end
    m_fee = m_fee + (fill.fee or 0)
end

function save_state(ctx)
    if balance_price ~= nil then
        ctx:state_set("balance_price", string.format("%.10f", balance_price))
    end
    ctx:state_set("built", built and "1" or "0")
    ctx:state_set("invested0", string.format("%.2f", invested0))
    ctx:state_set("entry_price", string.format("%.10f", entry_price))
    ctx:state_set("entry_size", string.format("%.10f", entry_size))
    -- 终态持久化(防重启复活, 同 shannon 审计教训)
    ctx:state_set("halted", halted and "1" or "0")
    -- 建仓单在途标记(防重启重复建仓)
    ctx:state_set("pending_entry", pending_entry and "1" or "0")
end

function on_init(ctx)
    local pair = ctx:config_str("pair")
    quote_asset = exec.detect_quote(pair)
    -- 数据需求声明(策略 → 引擎): 主时钟 + ATR 序列(周期与根数由策略显式给出)。
    ctx:need_klines("primary", ctx:config_str("interval") ~= nil
        and ctx:config_str("interval") ~= "" and ctx:config_str("interval") or "1h", 1000)
    local atr_need = math.floor(num(ctx, "atr_period", 14)) + 1
    if atr_need < 24 then
        atr_need = 24
    end
    local atr_intv = ctx:config_str("atr_interval")
    if atr_intv == nil or atr_intv == "" then
        atr_intv = "1h"
    end
    ctx:need_klines("aux", atr_intv, atr_need)

    built = false
    balance_price = nil
    pending_entry = false
    need_rehang = false
    halted = false
    cost_checked = false
    m_cash = nil
    m_pos = nil
    m_fee = 0

    -- 断点续接: 引擎已把上次会话的状态注入(表 strategy_state)
    local bp = ctx:state_get("balance_price")
    if bp ~= nil and tonumber(bp) and tonumber(bp) > 0 then
        balance_price = tonumber(bp)
        built = true
        invested0 = tonumber(ctx:state_get("invested0")) or 0
        entry_price = tonumber(ctx:state_get("entry_price")) or 0
        entry_size = tonumber(ctx:state_get("entry_size")) or 0
        -- 现金与持仓从引擎读(不冗余存), 模型账本与引擎重新同步
        m_cash = ctx:balance(quote_asset) or 0
        m_pos = ctx:position_size(pair) or 0
        -- 重启后必须重挂(停机撤单兜底已清掉本策略挂单)
        need_rehang = true
        pending_entry = (ctx:state_get("pending_entry") == "1")
        halted = (ctx:state_get("halted") == "1")
        if halted then
            ctx:log("[uniswap_v2_grid] 续接: 上次已停机(halted=true) -> 本次不再交易")
        end
        ctx:log(string.format(
            "[uniswap_v2_grid] 续接上次状态: 平衡价 %.4f, 投入 %.2f, 建仓 %.6f@%.4f",
            balance_price, invested0, entry_size, entry_price))
    end

    ctx:log(string.format(
        "[uniswap_v2_grid] init pair=%s quote=%s 投入=%s 建仓=投入一半 主时钟=%s ATR=%s×%d mult=%.2f " ..
        "min_notional=%.2f fee_side=%.4f",
        pair, quote_asset,
        (ctx:config_f64("invest_cash") or 0) > 0 and string.format("%.2f", ctx:config_f64("invest_cash"))
            or "激活时余额全额",
        ctx:config_str("interval") ~= nil and ctx:config_str("interval") ~= ""
            and ctx:config_str("interval") or "1h",
        atr_intv, num(ctx, "atr_period", 14), num(ctx, "atr_mult", 1),
        num(ctx, "min_notional", 5), num(ctx, "fee_side", 0.001)))
    ctx:log(string.format(
        "[uniswap_v2_grid] 激活: 价格低于 start_price=%.4f 才激活, 用投入一半市价建仓, " ..
        "成交价 = 第一平衡价格; 平衡价 ± ATR 挂单, 成交后 1:1 恢复并重挂",
        num(ctx, "start_price", 0)))
end

function on_tick(ctx)
    if halted then
        return {}
    end
    local pair = ctx:config_str("pair")

    -- 决策节流: 只在主时钟新 bar 时决策一次(用 ctx:now 的 open_time, 避免每 tick 全量克隆 klines)
    local t = ctx:now()
    if t == nil or t.ts == nil then
        return {}
    end
    if t.ts == last_bar_ts then
        return {}
    end
    last_bar_ts = t.ts

    local price = ctx:price(pair)
    last_price = price or last_price
    if not price or price <= 0 then
        return {}
    end

    local atr_mult = num(ctx, "atr_mult", 1)
    local min_notional = num(ctx, "min_notional", 5)
    local fee_side = num(ctx, "fee_side", 0.001)
    local fee_bps = fee_side * 10000
    local f = fee_side

    -- ATR(动态间距之源); 未就绪不挂单不猜值
    local atr_intv = ctx:config_str("atr_interval")
    if atr_intv == nil or atr_intv == "" then
        atr_intv = "1h"
    end
    local atr_period = math.floor(num(ctx, "atr_period", 14))
    if atr_period <= 0 then
        atr_period = 14
    end
    local atr = ctx:atr_tf(pair, atr_intv, atr_period)
    if not atr or atr <= 0 then
        skip_no_atr = skip_no_atr + 1
        stall_bars = stall_bars + 1
        if stall_since == nil then
            stall_since = t.ts
        end
        if t.ts - stall_since >= 86400000 and (stall_warned_at == nil or t.ts - stall_warned_at >= 86400000) then
            stall_warned_at = t.ts
            ctx:log(string.format(
                "[uniswap_v2_grid] [WARN] 停摆 ≥1 天: ATR 未就绪已连续跳过 %d 根 bar, 不挂单",
                skip_no_atr))
        end
        return {}
    end
    -- 生效间距 = max(atr_mult×ATR, 最小间距下限×价格); 下限防 ATR 过窄被手续费磨损
    local spacing = atr_mult * atr
    local min_spacing = num(ctx, "min_spacing_pct", 0.004) * price
    if spacing < min_spacing then
        spacing = min_spacing
    end

    -- 成本门槛(启动硬校验一次, 同 shannon R7): 生效间距必须 ≥ 4×单边费率(严格小于才停机;
    -- 默认 0.4% 下限恰等门槛, 往返毛利 0.2% > 0, 放行)
    if not cost_checked then
        cost_checked = true
        local need = 4 * fee_side * price
        if spacing < need then
            halted = true
            save_state(ctx)
            ctx:log(string.format(
                "[uniswap_v2_grid] [FATAL] 成本门槛不满足 -> 停机: 生效间距=%.6f (=%.4f%% 价格) " ..
                "必须 ≥ 4×fee_side×价格=%.6f (=%.4f%%); atr_mult=%.2f ATR=%.6f 价格=%.4f",
                spacing, spacing / price * 100, need, need / price * 100, atr_mult, atr, price))
            return {}
        end
        ctx:log(string.format(
            "[uniswap_v2_grid] 成本门槛通过(首次校验): 生效间距=%.6f (%.4f%% 价格) ≥ %.4f%%",
            spacing, spacing / price * 100, 4 * fee_side * 100))
    end

    -- ── 未激活: 价格 < start_price 才激活
    if not built then
        if pending_entry then
            return {} -- 建仓市价单已发出, 等成交回调
        end
        local start_px = num(ctx, "start_price", 0)
        if start_px <= 0 then
            halted = true
            save_state(ctx)
            ctx:log("[uniswap_v2_grid] [FATAL] 缺少必填参数 start_price(开始价格) -> 停机: " ..
                "策略只在价格低于 start_price 时激活。")
            return {}
        end
        if price >= start_px then
            return {}
        end
        -- 激活: 投入资金一半市价建仓(invest_cash=0 → 激活时 quote 余额全额视为投入)
        local inv = ctx:config_f64("invest_cash") or 0
        if inv <= 0 then
            inv = ctx:balance(quote_asset) or 0
        end
        invested0 = inv
        local amt = inv / 2
        local size = cap_buy(ctx, amt / price, price, fee_bps)
        if size > 0 and size * price >= min_notional then
            pending_entry = true
            ctx:log(string.format(
                "[uniswap_v2_grid] 激活(价 %.4f < 开始价 %.4f) -> 市价买入 %.6f (≈%.2f %s, 投入 %.2f 的一半), " ..
                "成交价将成为第一平衡价格",
                price, start_px, size, size * price, quote_asset, inv))
            return {
                { pair = pair, side = "buy", size = size, order_type = "market" },
            }
        end
        ctx:log(string.format(
            "[uniswap_v2_grid] 激活但建仓名义 %.2f < min_notional %.2f -> 按无建仓处理(平衡价 := 现价)",
            size * price, min_notional))
        built = true
        balance_price = price
        need_rehang = true
        save_state(ctx)
        return {}
    end

    -- ── 已激活
    if balance_price == nil then
        return {}
    end

    -- 停摆恢复(此前在途/未就绪, 现在可以重挂了)
    if stall_since ~= nil then
        stall_since = nil
        stall_warned_at = nil
    end

    if not need_rehang then
        return {} -- 两侧单未成交 → 平衡价不变, 不重挂
    end

    -- 重挂(全撤重挂): C/Q 取真实账本, 量按"成交后 1:1 恢复"(含费修正)
    local C = ctx:balance(quote_asset) or 0
    local Q = ctx:position_size(pair) or 0
    local buy_px = balance_price - spacing
    local sell_px = balance_price + spacing
    local orders = { { pair = pair, action = "cancel_pending" } }

    if buy_px > 0 then
        local q = (C - Q * buy_px) / (buy_px * (2 + f))
        if q > 0 then
            q = cap_buy(ctx, q, buy_px, fee_bps)
            if q * buy_px >= min_notional then
                orders[#orders + 1] = {
                    pair = pair, side = "buy", size = q, order_type = "limit", price = buy_px,
                }
            end
        end
    else
        skip_zero_buy_px = skip_zero_buy_px + 1
    end

    if Q > 0 then
        local q = (Q * sell_px - C) / (sell_px * (2 - f))
        if q > 0 then
            q = cap_sell(ctx, pair, q)
            if q * sell_px >= min_notional then
                orders[#orders + 1] = {
                    pair = pair, side = "sell", size = q, order_type = "limit", price = sell_px,
                }
            end
        end
    end

    if #orders <= 1 then
        -- 两侧都没挂出(量太小/名义不足/单侧资金耗尽): 保持 need_rehang 下一 tick 再试(防永久停摆)
        skip_notional = skip_notional + 1
        stall_bars = stall_bars + 1
        if stall_since == nil then
            stall_since = t.ts
        end
        if t.ts - stall_since >= 86400000 and (stall_warned_at == nil or t.ts - stall_warned_at >= 86400000) then
            stall_warned_at = t.ts
            ctx:log(string.format(
                "[uniswap_v2_grid] [WARN] 停摆 ≥1 天: 平衡价 %.4f 间距 %.4f 下两侧均无法挂单 " ..
                "(现金 %.2f 持仓 %.6f, 小名义跳过 %d 次)",
                balance_price, spacing, C, Q, skip_notional))
        end
        return {}
    end

    need_rehang = false
    rehang_count = rehang_count + 1
    save_state(ctx)
    ctx:log(string.format(
        "[uniswap_v2_grid] 网格重挂 #%d: 平衡价 %.4f 间距 %.4f (ATR %.4f×%.2f) -> 买 %s / 卖 %s | " ..
        "现金 %.2f 持仓 %.6f",
        rehang_count, balance_price, spacing, atr, atr_mult,
        buy_px > 0 and string.format("%.4f", buy_px) or "跳过(买价≤0)",
        Q > 0 and string.format("%.4f", sell_px) or "无(无持仓)",
        C, Q))
    return orders
end

function on_fill(ctx, fill)
    local px = fill.fill_price or balance_price or 0
    local size = fill.fill_size or 0

    if pending_entry then
        -- 建仓成交: 该笔成交价 = 第一平衡价格; 模型账本在此与引擎同步一次
        pending_entry = false
        built = true
        balance_price = px
        entry_price = px
        entry_size = size
        m_cash = ctx:balance(quote_asset) or 0
        m_pos = ctx:position_size(ctx:config_str("pair")) or 0
        m_fee = fill.fee or 0
        -- 建仓也是真实成交: 计入成交统计(规范 §B)
        if fill.side == "buy" then
            buy_count = buy_count + 1
        else
            sell_count = sell_count + 1
        end
        fill_count = fill_count + 1
        need_rehang = true
        save_state(ctx)
        ctx:log(string.format(
            "[uniswap_v2_grid] 建仓成交 %.6f @ %.4f (费 %.4f) -> 平衡价 := %.4f | 现金 %.2f 持仓 %.6f " ..
            "仓位市值 %.2f (投入 %.2f)",
            size, px, fill.fee or 0, balance_price, m_cash, m_pos, m_pos * px, invested0))
        return
    end

    -- 网格成交: 平衡价 := 成交价; 模型账本按真实成交推进; 全撤重挂
    balance_price = px
    model_apply(fill, px, size)
    if fill.side == "buy" then
        buy_count = buy_count + 1
    else
        sell_count = sell_count + 1
    end
    fill_count = fill_count + 1
    need_rehang = true
    save_state(ctx)
    ctx:log(string.format(
        "[FILL] #%d %s size=%.6f px=%.4f notional=%.2f 费=%.4f | 现金 %.2f 持仓 %.6f " ..
        "平衡价(新)=%.4f 1:1 检查: 现金/仓位市值 = %.4f",
        fill_count, fill.side, size, px, size * px, fill.fee or 0,
        m_cash or 0, m_pos or 0, balance_price,
        (m_pos and m_pos > 0 and m_pos * px > 0) and (m_cash or 0) / (m_pos * px) or 0))
end

-- 拒单回传: 重挂防停摆。
-- 只处理 rejected; cancelled 是策略主动撤单(cancel_pending)的正常回传, 重挂已在 on_tick 里处理。
function on_order_update(ctx, upd)
    local status = upd and upd.status
    if status == "rejected" then
        if pending_entry then
            pending_entry = false -- 建仓被拒: 恢复, 下个 tick 重新走激活
        end
        need_rehang = true
        ctx:log(string.format(
            "[uniswap_v2_grid] 挂单被拒: pair=%s filled=%.6f remaining=%.6f → 重挂",
            upd.pair, upd.filled_size or 0, upd.remaining_size or 0))
    end
end

function on_stop(ctx)
    local pair = ctx:config_str("pair")
    local pos = ctx:position_size(pair) or 0
    local cash = ctx:balance(quote_asset) or 0
    local eq = ctx:equity() or 0

    -- 账本交叉核对(规范 §C.4): 模型 vs 引擎, 误差 > 0.01 → WARN
    local d_cash = (m_cash or cash) - cash
    local d_pos = (m_pos or pos) - pos
    if math.abs(d_cash) > 0.01 or math.abs(d_pos) > 0.01 then
        ctx:log(string.format(
            "[uniswap_v2_grid] [WARN] 账本分叉: 模型现金 %.6f vs 引擎 %.6f (差 %.6f), " ..
            "模型持仓 %.8f vs 引擎 %.8f (差 %.8f)",
            m_cash or 0, cash, d_cash, m_pos or 0, pos, d_pos))
    end

    -- 收益分解: 合计 = 期末权益 − 投入 = 持仓收益(期末持仓×(末价−建仓价)) + 交易收益(再平衡净贡献)
    local out = string.format(
        "[uniswap_v2_grid] 停机(**不清仓**): 成交 %d(买 %d 卖 %d) / 跳过(ATR未就绪 %d, 小名义 %d, 买价≤0 %d) / " ..
        "重挂 %d / %s / 平衡价 %s / 期末现金 %.2f 持仓 %.6f(市值 %.2f) 权益 %.2f / 模型费 %.4f / 账本差(现金 %.6f 持仓 %.8f)",
        fill_count, buy_count, sell_count, skip_no_atr, skip_notional, skip_zero_buy_px,
        rehang_count,
        halted and "已停机(成本门槛)" or "正常运行",
        balance_price and string.format("%.4f", balance_price) or "nil",
        cash, pos, pos * (last_price or 0), eq, m_fee, d_cash, d_pos)
    if invested0 > 0 and last_price > 0 then
        local total = eq - invested0
        local hold = 0
        if entry_price > 0 then
            hold = pos * (last_price - entry_price)
        end
        out = out .. string.format(
            " | 收益分解: 投入 %.2f | 合计 %+.2f = 持仓 %+.2f + 交易(再平衡) %+.2f",
            invested0, total, hold, total - hold)
    end
    ctx:log(out)

    -- 规范 §C.1: stat_* 导出(报告"策略统计"区块)
    ctx:state_set("stat_eff_atr_interval", ctx:config_str("atr_interval") or "1h")
    ctx:state_set("stat_eff_atr_period",
        string.format("%d", math.floor(num(ctx, "atr_period", 14))))
    ctx:state_set("stat_eff_atr_mult", string.format("%.4f", num(ctx, "atr_mult", 1)))
    ctx:state_set("stat_eff_min_spacing_pct",
        string.format("%.4f", num(ctx, "min_spacing_pct", 0.004)))
    ctx:state_set("stat_eff_min_notional", string.format("%.2f", num(ctx, "min_notional", 5)))
    ctx:state_set("stat_eff_fee_side", string.format("%.4f", num(ctx, "fee_side", 0.001)))
    ctx:state_set("stat_invested0", string.format("%.2f", invested0))
    ctx:state_set("stat_entry_price", string.format("%.6f", entry_price))
    ctx:state_set("stat_entry_size", string.format("%.6f", entry_size))
    ctx:state_set("stat_fill_count", string.format("%d", fill_count))
    ctx:state_set("stat_buy_count", string.format("%d", buy_count))
    ctx:state_set("stat_sell_count", string.format("%d", sell_count))
    ctx:state_set("stat_rehang_count", string.format("%d", rehang_count))
    ctx:state_set("stat_skip_no_atr", string.format("%d", skip_no_atr))
    ctx:state_set("stat_skip_notional", string.format("%d", skip_notional))
    ctx:state_set("stat_stall_bars", string.format("%d", stall_bars))
    ctx:state_set("stat_cash_final", string.format("%.2f", cash))
    ctx:state_set("stat_pos_final", string.format("%.8f", pos))
    ctx:state_set("stat_pos_value_final", string.format("%.2f", pos * (last_price or 0)))
    ctx:state_set("stat_balance_price",
        balance_price and string.format("%.6f", balance_price) or "")
    ctx:state_set("stat_fee_total", string.format("%.4f", m_fee))
    ctx:state_set("stat_ledger_diff_cash", string.format("%.6f", d_cash))
    ctx:state_set("stat_ledger_diff_pos", string.format("%.8f", d_pos))
    ctx:state_set("stat_halted", halted and "1" or "0")
end
