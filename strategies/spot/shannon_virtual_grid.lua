-- 现货虚拟香农网格 (shannon_virtual_grid)
--
-- ═══ 语义 ═══
--   虚拟账本(v_cash + v_pos)恒 1:1 的香农再平衡网格, 以 EMA 金叉/死叉作为交易闸门、
--   网格间距作为最小漂移带: 只在交叉事件发生且价格偏离平衡价 ≥ 一个间距时, 按虚拟账本
--   1:1 恢复量下**市价单**再平衡 —— 平时盘口不挂任何限价单。
--   金叉(趋势转多)且价格低于平衡价 → 买入回补; 死叉(趋势转空)且价格高于平衡价 → 卖出减仓。
--   虚拟倍数 1~5(默认 2): 总资金 v_total = 投入 × 倍数, 虚拟仓位按 v_total 的 1:1 建立。
--   现货无真实杠杆: 实际买入受可用现金截断(虚拟目标量 > 现金时实发只到现金上限, 计入 cash_limited),
--   有效杠杆随交易自然回落。
--   方向 flag(卖笔数 − 买笔数, 建仓/重置不计)驱动趋势侧间距指数放大:
--   |flag|≥2 时 flag<0 放大买间距、flag>0 放大卖间距, 间距 × flag_spacing_mult^(|flag|−1),
--   另一侧不变; 回调成交即 flag 回退、放大自收缩。
--   死叉卖量超过实际持仓 → 不部分卖出: 按初始投入 × 倍数在当前价重建 1:1 虚拟仓位,
--   平衡价 := 当前价, 本次不发单(重置不计 flag)。
--   建仓开关(enable_build, 默认关): 打开时等待价格低于 build_price, 市价买入 build_amount
--   建立实际仓位, 成交后策略正式开始; 关闭时首次金叉只做虚拟初始化(平衡价=现价+间距,
--   按总资金 1:1 建虚拟仓位), 不下任何实际订单。
--   ⚠ 单边暴跌套牢风险: 建议用认沽期权/做空合约对冲尾部。
--
-- ═══ 双账本 ═══
--   虚拟账本 v_cash/v_pos = 决策引擎, 按目标挂单量推进, 恒满足 v_cash = v_pos × 平衡价;
--   真实账本 m_cash/m_pos = 按实际成交推进, 停机与引擎余额/持仓交叉核对(误差 < 0.01)。
--   买量被真实现金封顶时两账本合法分叉(v_pos > m_pos), 该分叉正是死叉卖量超持仓 → 重置的来源。
--
-- ═══ 参数 ═══
--   pair              必填, 交易对(现货)
--   invest_cash       投入资金, 默认 0(= 启动时 quote 余额全额); v_total = invest_cash × virtual_mult
--   virtual_mult      虚拟倍数, 默认 2, 合法 1~5(越界停机)
--   enable_build      建仓开关, 默认 false
--   build_price       建仓触发价(enable_build 必填, 价格低于它才市价建仓)
--   build_amount      建仓金额 USDT(enable_build 必填)
--   interval          主时钟, 默认 1m
--   ema_interval      EMA K 线周期, 默认 1m
--   ema_fast          EMA 快线周期, 默认 2
--   ema_slow          EMA 慢线周期, 默认 3
--   atr_interval      ATR K 线周期, 默认 1m
--   atr_period        ATR 周期, 默认 14
--   atr_mult          间距 = atr_mult × ATR, 默认 5
--   min_spacing_pct   最小间距, 默认 0.004(0.4%); 生效间距 = max(atr_mult×ATR, 下限×价格); 负数禁用下限
--   flag_spacing_mult 趋势侧间距放大系数, 默认 1.2; 1.0 = 禁用(<1 按 1 处理)
--   min_notional      单笔最小名义, 默认 5
--   fee_side          单边费率, 默认 0.001(现货 0.1%/边)
--
-- ═══ 守卫 ═══
--   ATR 未就绪 → 不动作(计数); 生效间距 < 4×fee_side×价格 → [FATAL] 停机(启动校验一次);
--   建仓后连续 1 天无任何成交 → WARN(停摆可观测性)。
--
-- ═══ 用法 ═══
--   ricow backtest --strategy shannon_virtual_grid --pair SOLUSDT --interval 1m --days 180 \
--     --cash 10000 --param atr_mult=5
--   ricow run shannon_virtual_grid --demo

quote_asset = ""  -- 计价资产, on_init 里 detect_quote(pair) 动态检测
-- 状态
built = false
balance_price = nil       -- 平衡价格(虚拟初始化价/建仓成交价/网格成交价, 或重置时的当前价)
pending_entry = false     -- 建仓市价单在途
pending_clip = 0          -- 本次市价单对应的虚拟目标量(on_tick 写, on_fill 读)
halted = false            -- 停机标记(参数/成本门槛)
fatal = 0                 -- 数值化停机标记(单测经 global_f64 读取)
cost_checked = false      -- 成本门槛只做一次启动校验
last_bar_ts = nil
invested0 = 0            -- 投入资金(定格基准; invest_cash=0 时延迟到激活时刻取余额)
invest_resolved = false   -- 投入是否已定格(一次性)
virtual_mult = 2
v_total = 0               -- 总资金 = 投入 × 倍数(启动时刻定格)
entry_price = 0           -- 初始建仓成交价(持仓损益基准)
entry_size = 0            -- 初始建仓数量
last_price = 0
-- 双账本
v_cash = nil              -- 虚拟现金(决策引擎)
v_pos = nil               -- 虚拟持仓(决策引擎)
m_cash = nil              -- 真实现金(按实际成交推进, 与引擎对账)
m_pos = nil               -- 真实持仓(按实际成交推进, 与引擎对账)
m_fee = 0                 -- 真实账本累计手续费(Σ fill.fee)
-- EMA 交叉状态
ema_prev_fast = nil
ema_prev_slow = nil
-- 方向 flag
flag = 0
flag_max = 0
flag_min = 0
-- 间距可观测(每决策 bar 刷新)
last_spacing = 0
last_buy_spacing = 0
last_sell_spacing = 0
-- 计数
fill_count = 0
buy_count = 0
sell_count = 0
skip_no_atr = 0
skip_notional = 0
reset_count = 0           -- 死叉卖量超持仓 → 重置虚拟仓位次数
cash_limited_buys = 0      -- 买单被可用现金截断次数(虚拟目标量 > 现金, 实发只到现金上限)
cash_limited_notional = 0  -- 上述被截断的累计名义
cash_limited_warned = false -- 截断 WARN 去重(解除后重新告警)
-- 停摆可观测性
stall_bars = 0
stall_bar_ts = nil
stall_since = nil
stall_warned_at = nil
last_fill_ts = nil

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

-- 买量现金兜底(预扣手续费 + 滑点余量, 同现货 shannon 口径)
local function cap_buy(ctx, size, p, fee_bps)
    local cash = ctx:balance(quote_asset) or 0
    local max_buy = cash / (p * (1 + (fee_bps + 20) / 10000))
    if size > max_buy then
        return max_buy
    end
    return size
end

-- 方向 flag 间距指数放大: 返回 (buy_spacing, sell_spacing)。
--   指数 e = max(|flag|−1, 0): |flag|≤1 两侧均不放大; flag<0(净买) → 买间距 × mult^e;
--   flag>0(净卖) → 卖间距 × mult^e。mult < 1 钳到 1(= 禁用放大)。
local function side_spacings(base_spacing, flag_, mult)
    if mult < 1 then
        mult = 1
    end
    local e = math.abs(flag_) - 1
    if e < 0 then
        e = 0
    end
    if flag_ < 0 then
        return base_spacing * mult^e, base_spacing
    elseif flag_ > 0 then
        return base_spacing, base_spacing * mult^e
    end
    return base_spacing, base_spacing
end

-- 真实账本按实际成交推进(fill.fee 已含, 引擎同源口径)
local function model_apply(fill, px, size)
    if m_cash == nil then
        return -- 建仓成交在 on_fill 里同步初始值, 不会走到这里
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
    ctx:state_set("virtual_mult", string.format("%.4f", virtual_mult))
    ctx:state_set("entry_price", string.format("%.10f", entry_price))
    ctx:state_set("entry_size", string.format("%.10f", entry_size))
    if v_cash ~= nil then
        ctx:state_set("v_cash", string.format("%.10f", v_cash))
    end
    if v_pos ~= nil then
        ctx:state_set("v_pos", string.format("%.10f", v_pos))
    end
    ctx:state_set("m_fee", string.format("%.10f", m_fee))
    ctx:state_set("flag", string.format("%d", flag))
    ctx:state_set("halted", halted and "1" or "0")
    ctx:state_set("pending_entry", pending_entry and "1" or "0")
    ctx:state_set("reset_count", string.format("%d", reset_count))
    ctx:state_set("cash_limited_buys", string.format("%d", cash_limited_buys))
    ctx:state_set("cash_limited_notional", string.format("%.4f", cash_limited_notional))
    if ema_prev_fast ~= nil then
        ctx:state_set("ema_prev_fast", string.format("%.10f", ema_prev_fast))
    end
    if ema_prev_slow ~= nil then
        ctx:state_set("ema_prev_slow", string.format("%.10f", ema_prev_slow))
    end
end

function on_init(ctx)
    local pair = ctx:config_str("pair")
    quote_asset = exec.detect_quote(pair)
    -- 数据需求声明: 主时钟 + ATR/EMA 序列(周期与根数由策略显式给出)。
    local main_intv = cfg_str(ctx, "interval", "1m")
    ctx:need_klines("primary", main_intv, 1000)
    local atr_intv = cfg_str(ctx, "atr_interval", "1m")
    local ema_intv = cfg_str(ctx, "ema_interval", "1m")
    local atr_period = math.floor(num(ctx, "atr_period", 14))
    if atr_period <= 0 then
        atr_period = 14
    end
    local ema_slow = math.floor(num(ctx, "ema_slow", 3))
    if ema_slow <= 0 then
        ema_slow = 3
    end
    -- 预热根数 = ATR 需求与 EMA 需求(首值种, 3×(slow+1) 收敛带)取大, 下限 60。
    local need = math.max(atr_period + 1, 3 * (ema_slow + 1), 60)
    if ema_intv == atr_intv then
        ctx:need_klines("aux", atr_intv, need)
    else
        ctx:need_klines("aux", atr_intv, atr_period + 1)
        ctx:need_klines("aux", ema_intv, 3 * (ema_slow + 1))
    end

    built = false
    balance_price = nil
    pending_entry = false
    pending_clip = 0
    halted = false
    fatal = 0
    cost_checked = false
    v_cash = nil
    v_pos = nil
    m_cash = nil
    m_pos = nil
    m_fee = 0
    ema_prev_fast = nil
    ema_prev_slow = nil
    flag = 0
    flag_max = 0
    flag_min = 0
    reset_count = 0
    cash_limited_buys = 0
    cash_limited_notional = 0
    invest_resolved = false

    -- 投入与总资金: invest_cash>0 启动定格; =0 延迟到激活时刻取 quote 余额(on_init 快照未就绪)。
    invested0 = ctx:config_f64("invest_cash") or 0
    virtual_mult = num(ctx, "virtual_mult", 2)
    if invested0 > 0 then
        v_total = invested0 * virtual_mult
    end

    -- 参数校验: 虚拟倍数 1~5
    if virtual_mult < 1 or virtual_mult > 5 then
        halted = true
        fatal = 1
        ctx:log(string.format(
            "[shannon_virtual_grid] [FATAL] 虚拟倍数越界 -> 停机: virtual_mult=%.2f 必须在 1~5 之间(默认 2)",
            virtual_mult))
        return
    end
    -- 参数校验: 建仓开关打开时 build_price/build_amount 必填
    local enable_build = ctx:config_bool("enable_build")
    if enable_build then
        local bp = num(ctx, "build_price", 0)
        local ba = num(ctx, "build_amount", 0)
        if bp <= 0 or ba <= 0 then
            halted = true
            fatal = 1
            ctx:log(string.format(
                "[shannon_virtual_grid] [FATAL] 建仓参数缺失 -> 停机: enable_build=true 时 " ..
                "build_price=%.4f build_amount=%.2f 必须均 > 0", bp, ba))
            return
        end
    end

    -- 断点续接: 引擎已把上次会话的状态注入(表 strategy_state)
    local bp = ctx:state_get("balance_price")
    if bp ~= nil and tonumber(bp) and tonumber(bp) > 0 then
        balance_price = tonumber(bp)
        built = true
        invested0 = tonumber(ctx:state_get("invested0")) or invested0
        virtual_mult = tonumber(ctx:state_get("virtual_mult")) or virtual_mult
        v_total = invested0 * virtual_mult
        entry_price = tonumber(ctx:state_get("entry_price")) or 0
        entry_size = tonumber(ctx:state_get("entry_size")) or 0
        v_cash = tonumber(ctx:state_get("v_cash"))
        v_pos = tonumber(ctx:state_get("v_pos"))
        m_fee = tonumber(ctx:state_get("m_fee")) or 0
        flag = tonumber(ctx:state_get("flag")) or 0
        reset_count = tonumber(ctx:state_get("reset_count")) or 0
        cash_limited_buys = tonumber(ctx:state_get("cash_limited_buys")) or 0
        cash_limited_notional = tonumber(ctx:state_get("cash_limited_notional")) or 0
        local ef = ctx:state_get("ema_prev_fast")
        local es = ctx:state_get("ema_prev_slow")
        ema_prev_fast = ef and tonumber(ef) or nil
        ema_prev_slow = es and tonumber(es) or nil
        -- 真实账本与引擎重新同步(不冗余存)
        m_cash = ctx:balance(quote_asset) or 0
        m_pos = ctx:position_size(pair) or 0
        if v_cash == nil or v_pos == nil or v_cash <= 0 or v_pos <= 0 then
            halted = true
            fatal = 1
            save_state(ctx)
            ctx:log("[shannon_virtual_grid] [FATAL] 续接失败: 已建仓但状态中无有效虚拟账本 " ..
                "(v_cash/v_pos 缺失) -> 停机, 请人工核对持仓。")
            return
        end
        pending_entry = (ctx:state_get("pending_entry") == "1")
        halted = (ctx:state_get("halted") == "1")
        if halted then
            ctx:log("[shannon_virtual_grid] 续接: 上次已停机(halted=true) -> 本次不再交易")
        end
        ctx:log(string.format(
            "[shannon_virtual_grid] 续接上次状态: 平衡价 %.4f, 投入 %.2f×%.1f, 虚拟现金 %.2f 虚拟持仓 %.6f, flag=%d",
            balance_price, invested0, virtual_mult, v_cash, v_pos, flag))
    end

    ctx:log(string.format(
        "[shannon_virtual_grid] init pair=%s quote=%s 投入=%s 虚拟倍数=%.2f 主时钟=%s " ..
        "EMA=%s %d/%d ATR=%s×%d mult=%.2f min_spacing=%.4f flag_mult=%.2f min_notional=%.2f fee_side=%.4f",
        pair, quote_asset,
        invested0 > 0 and string.format("%.2f(总资金=%.2f)", invested0, v_total)
            or "0(激活时取余额)",
        virtual_mult, main_intv,
        ema_intv, math.floor(num(ctx, "ema_fast", 2)), ema_slow,
        atr_intv, atr_period, num(ctx, "atr_mult", 5),
        num(ctx, "min_spacing_pct", 0.004), num(ctx, "flag_spacing_mult", 1.2),
        num(ctx, "min_notional", 5), num(ctx, "fee_side", 0.001)))
    if enable_build then
        ctx:log(string.format(
            "[shannon_virtual_grid] 建仓开关: 价格低于 build_price=%.4f 时市价买入 %.2f %s, " ..
            "成交价 = 第一平衡价格, 建仓后策略正式开始",
            num(ctx, "build_price", 0), num(ctx, "build_amount", 0), quote_asset))
    else
        ctx:log("[shannon_virtual_grid] 建仓开关关闭: 首次金叉只做虚拟初始化(平衡价=现价+间距, 1:1 虚拟仓位), 不下实际订单")
    end
    ctx:log("[shannon_virtual_grid] ⚠ 现货无真实杠杆: mult>1 仅放大虚拟账本, 实际买入受现金封顶; " ..
        "单边暴跌会套牢, 建议用认沽期权/做空合约对冲尾部风险")
end

-- 死叉卖量超实际持仓 → 按初始投入×倍数在当前价重建 1:1 虚拟仓位(不发单、不计 flag)。
local function reset_virtual(ctx, pair, price, q_v, real_pos)
    v_total = invested0 * virtual_mult
    v_pos = v_total / (2 * price)
    v_cash = v_total / 2
    balance_price = price
    reset_count = reset_count + 1
    save_state(ctx)
    ctx:log(string.format(
        "[shannon_virtual_grid] 重置虚拟仓位 #%d: 死叉卖量 %.6f > 实际持仓 %.6f -> " ..
        "按投入 %.2f × 倍数 %.2f 重建 1:1 @%.4f (v_pos %.6f v_cash %.2f), 本次不发单",
        reset_count, q_v, real_pos, invested0, virtual_mult, price, v_pos, v_cash))
end

function on_tick(ctx)
    if halted then
        return {}
    end
    local pair = ctx:config_str("pair")

    -- 决策节流: 只在主时钟新 bar 时决策一次
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

    local min_notional = num(ctx, "min_notional", 5)
    local fee_side = num(ctx, "fee_side", 0.001)
    local fee_bps = fee_side * 10000
    local f = fee_side

    -- ATR(动态间距之源); 未就绪不动作不猜值。
    -- atr_interval = 主时钟时走 ctx:atr(primary 尾窗, 每 bar 固定尾窗重算):
    -- Wilder ATR 早期 TR 权重按 (13/14)^k 衰减, 1000 根尾窗残差 <1e-13, 与全前缀语义一致;
    -- 跨周期才用 ctx:atr_tf(TfCache 缓存 + 尾窗)。
    local atr_intv = cfg_str(ctx, "atr_interval", "1m")
    local main_intv = cfg_str(ctx, "interval", "1m")
    local atr_period = math.floor(num(ctx, "atr_period", 14))
    if atr_period <= 0 then
        atr_period = 14
    end
    local atr
    if atr_intv == main_intv then
        atr = ctx:atr(pair, atr_period)
    else
        atr = ctx:atr_tf(pair, atr_intv, atr_period)
    end
    if not atr or atr <= 0 then
        skip_no_atr = skip_no_atr + 1
        if t.ts ~= stall_bar_ts then
            stall_bar_ts = t.ts
            stall_bars = stall_bars + 1
        end
        if stall_since == nil then
            stall_since = t.ts
        end
        if t.ts - stall_since >= 86400 and (stall_warned_at == nil or t.ts - stall_warned_at >= 86400) then
            stall_warned_at = t.ts
            ctx:log(string.format(
                "[shannon_virtual_grid] [WARN] 停摆 ≥1 天: ATR 未就绪已连续跳过 %d 根 bar, 不动作",
                skip_no_atr))
        end
        return {}
    end
    -- 生效间距 = max(atr_mult×ATR, 最小间距下限×价格); 下限防 ATR 过窄被手续费磨损
    local spacing = num(ctx, "atr_mult", 5) * atr
    local min_spacing = num(ctx, "min_spacing_pct", 0.004) * price
    if min_spacing < 0 then
        min_spacing = 0 -- 负数 = 禁用下限
    end
    if spacing < min_spacing then
        spacing = min_spacing
    end

    -- 成本门槛(启动硬校验一次): 生效间距必须 ≥ 4×单边费率(严格小于才停机)
    if not cost_checked then
        cost_checked = true
        local need = 4 * fee_side * price
        if spacing < need then
            halted = true
            fatal = 1
            save_state(ctx)
            ctx:log(string.format(
                "[shannon_virtual_grid] [FATAL] 成本门槛不满足 -> 停机: 生效间距=%.6f (=%.4f%% 价格) " ..
                "必须 ≥ 4×fee_side×价格=%.6f (=%.4f%%); ATR=%.6f 价格=%.4f",
                spacing, spacing / price * 100, need, need / price * 100, atr, price))
            return {}
        end
        ctx:log(string.format(
            "[shannon_virtual_grid] 成本门槛通过(首次校验): 生效间距=%.6f (%.4f%% 价格) ≥ %.4f%%",
            spacing, spacing / price * 100, 4 * fee_side * 100))
    end

    -- 方向 flag 间距放大: 生效间距之上按 mult^(|flag|−1) 放大趋势侧
    local buy_spacing, sell_spacing = side_spacings(spacing, flag, num(ctx, "flag_spacing_mult", 1.2))
    last_spacing = spacing
    last_buy_spacing = buy_spacing
    last_sell_spacing = sell_spacing

    -- EMA 交叉检测(引擎只给当前值, 前值由策略自持)。
    -- ema_interval = 主时钟时走 ctx:ema(primary 尾窗, 每 bar O(1000) 重算):
    -- EMA2/3 对 1000 根尾窗的首值种残差 (1/3)^1000 ≈ 0, 与全前缀重算语义一致;
    -- 跨周期才用 ctx:ema_cross(TfCache)。
    local main_intv = cfg_str(ctx, "interval", "1m")
    local ema_intv = cfg_str(ctx, "ema_interval", "1m")
    local ema_fast = math.floor(num(ctx, "ema_fast", 2))
    local ema_slow = math.floor(num(ctx, "ema_slow", 3))
    if ema_fast <= 0 then
        ema_fast = 2
    end
    if ema_slow <= 0 then
        ema_slow = 3
    end
    local cur_fast, cur_slow
    if ema_intv == main_intv then
        cur_fast = ctx:ema(pair, ema_fast)
        cur_slow = ctx:ema(pair, ema_slow)
        if cur_fast == nil or cur_slow == nil then
            return {} -- 指标未就绪, 不更新前值不判交叉
        end
    else
        local cur = ctx:ema_cross(pair, ema_intv, ema_fast, ema_slow)
        if cur == nil then
            return {} -- 指标未就绪, 不更新前值不判交叉
        end
        cur_fast = cur.fast
        cur_slow = cur.slow
    end
    local golden = false
    local death = false
    if ema_prev_fast ~= nil and ema_prev_slow ~= nil then
        golden = (ema_prev_fast <= ema_prev_slow) and (cur_fast > cur_slow)
        death = (ema_prev_fast >= ema_prev_slow) and (cur_fast < cur_slow)
    end
    ema_prev_fast = cur_fast
    ema_prev_slow = cur_slow

    -- 停摆恢复观测: 已建仓后距上次成交 ≥1 天 WARN(每日一条)
    if built and last_fill_ts ~= nil and t.ts - last_fill_ts >= 86400
        and (stall_warned_at == nil or t.ts - stall_warned_at >= 86400) then
        stall_warned_at = t.ts
        stall_bars = stall_bars + 1
        ctx:log(string.format(
            "[shannon_virtual_grid] [WARN] 停摆 ≥1 天: 平衡价 %.4f 间距 %.4f 下无交叉成交 " ..
            "(现金 %.2f 持仓 %.6f)", balance_price, spacing, ctx:balance(quote_asset) or 0,
            ctx:position_size(pair) or 0))
    end

    -- 投入延迟定格: invest_cash=0 时取激活时刻 quote 余额(on_init 快照未就绪), 一次性。
    if not invest_resolved and not built then
        invest_resolved = true
        if invested0 <= 0 then
            invested0 = ctx:balance(quote_asset) or 0
            v_total = invested0 * virtual_mult
            ctx:log(string.format(
                "[shannon_virtual_grid] 投入定格: 激活时刻余额 %.2f %s × 倍数 %.2f -> 总资金 %.2f",
                invested0, quote_asset, virtual_mult, v_total))
        end
    end

    -- ── 建仓开关: 等价格低于 build_price 市价建仓, 建仓前策略不运行
    local enable_build = ctx:config_bool("enable_build")
    if enable_build and not built then
        if pending_entry then
            return {} -- 建仓市价单在途, 等成交回调
        end
        if price >= num(ctx, "build_price", 0) then
            return {}
        end
        local ba = num(ctx, "build_amount", 0)
        local size = cap_buy(ctx, ba / price, price, fee_bps)
        if size > 0 and size * price >= min_notional then
            pending_entry = true
            pending_clip = v_total / (2 * price)
            ctx:log(string.format(
                "[shannon_virtual_grid] 建仓(价 %.4f < 触发价 %.4f) -> 市价买入 %.6f (≈%.2f %s), " ..
                "成交价将成为第一平衡价格",
                price, num(ctx, "build_price", 0), size, size * price, quote_asset))
            return {
                { pair = pair, side = "buy", size = size, order_type = "market" },
            }
        end
        ctx:log(string.format(
            "[shannon_virtual_grid] [WARN] 建仓名义 %.2f < min_notional %.2f -> 跳过建仓, 直接以现价开网格",
            size * price, min_notional))
        built = true
        balance_price = price
        v_pos = v_total / (2 * price)
        v_cash = v_total / 2
        m_cash = ctx:balance(quote_asset) or 0
        m_pos = ctx:position_size(pair) or 0
        last_fill_ts = t.ts
        save_state(ctx)
        return {}
    end

    -- ── 未建仓(建仓关闭): 首次金叉只做虚拟初始化, **不下任何实际订单**。
    --    当前价 + 网格间距 = 初始平衡价, 在平衡价按总资金 1:1 建立虚拟仓位。
    if not built then
        if pending_entry then
            return {}
        end
        if not golden then
            return {}
        end
        built = true
        balance_price = price + spacing
        v_pos = v_total / (2 * balance_price)
        v_cash = v_total / 2
        m_cash = ctx:balance(quote_asset) or 0
        m_pos = ctx:position_size(pair) or 0
        last_fill_ts = t.ts
        save_state(ctx)
        ctx:log(string.format(
            "[shannon_virtual_grid] 虚拟初始化(首次金叉, 现价 %.4f + 间距 %.4f) -> 平衡价 %.4f, " ..
            "按总资金 %.2f 建 1:1 虚拟仓位(虚拟持仓 %.6f 虚拟现金 %.2f) | 不下实际订单, 真实现金 %.2f 全额保留",
            price, spacing, balance_price, v_total, v_pos, v_cash, m_cash))
        return {}
    end

    -- ── 已建仓: 交叉 + 越带才市价再平衡
    if balance_price == nil or v_cash == nil or v_pos == nil then
        return {}
    end

    if golden and balance_price - price >= buy_spacing then
        local q_v = (v_cash - v_pos * price) / (price * (2 + f))
        if q_v > 0 then
            local size = cap_buy(ctx, q_v, price, fee_bps)
            if size < q_v * (1 - 1e-9) then
                cash_limited_buys = cash_limited_buys + 1
                cash_limited_notional = cash_limited_notional + (q_v - size) * price
                if not cash_limited_warned then
                    cash_limited_warned = true
                    ctx:log(string.format(
                        "[shannon_virtual_grid] [WARN] 买单被现金截断(去重, 后续同类不再刷屏): 虚拟目标 %.6f > 现金可买 %.6f " ..
                        "(名义差 %.2f) -> 实发截到现金上限, 虚拟账本仍按目标推进",
                        q_v, size, (q_v - size) * price))
                end
            else
                cash_limited_warned = false
            end
            if size > 0 and size * price >= min_notional then
                pending_clip = q_v
                return {
                    { pair = pair, side = "buy", size = size, order_type = "market" },
                }
            end
            skip_notional = skip_notional + 1
        end
        return {}
    end

    if death and price - balance_price >= sell_spacing then
        local q_v = (v_pos * price - v_cash) / (price * (2 - f))
        if q_v > 0 then
            local real_pos = ctx:position_size(pair) or 0
            if q_v > real_pos then
                reset_virtual(ctx, pair, price, q_v, real_pos)
                return {}
            end
            if q_v * price >= min_notional then
                pending_clip = q_v
                return {
                    { pair = pair, side = "sell", size = q_v, order_type = "market" },
                }
            end
            skip_notional = skip_notional + 1
        end
        return {}
    end

    return {}
end

function on_fill(ctx, fill)
    if halted then
        return {}
    end
    local pair = ctx:config_str("pair")
    local px = fill.fill_price or balance_price or 0
    local size = fill.fill_size or 0
    local t = ctx:now()
    last_price = px
    if t ~= nil and t.ts ~= nil then
        last_fill_ts = t.ts
    end

    if pending_entry then
        -- 建仓成交(enable_build 路径): 成交价 = 第一平衡价格; 虚拟账本按总资金精确 1:1 建立
        -- (与实际成交量无关, 实际量只进真实账本); 不计 flag。
        pending_entry = false
        pending_clip = 0
        built = true
        balance_price = px
        entry_price = px
        entry_size = size
        v_pos = v_total / (2 * px)
        v_cash = v_total / 2
        m_cash = ctx:balance(quote_asset) or 0
        m_pos = ctx:position_size(pair) or 0
        m_fee = fill.fee or 0
        if fill.side == "buy" then
            buy_count = buy_count + 1
        else
            sell_count = sell_count + 1
        end
        fill_count = fill_count + 1
        save_state(ctx)
        ctx:log(string.format(
            "[shannon_virtual_grid] 建仓成交 %.6f @ %.4f (费 %.4f) -> 平衡价 := %.4f | " ..
            "虚拟现金 %.2f 虚拟持仓 %.6f (总资金 %.2f = 投入 %.2f × 倍数 %.2f) | 真实持仓 %.6f",
            size, px, fill.fee or 0, balance_price, v_cash, v_pos, v_total, invested0, virtual_mult, m_pos))
        return {}
    end

    -- 网格成交: 平衡价 := 成交价; 虚拟账本按目标量推进(1:1 恒等式精确保持),
    -- 真实账本按实际成交推进; 买 −1 / 卖 +1 计 flag。
    local q = pending_clip
    if q <= 0 then
        q = size -- 防御: 状态丢失时退化为按实际量推进
    end
    pending_clip = 0
    balance_price = px
    local f = num(ctx, "fee_side", 0.001)
    if fill.side == "buy" then
        v_cash = v_cash - q * px * (1 + f)
        v_pos = v_pos + q
        flag = flag - 1
        buy_count = buy_count + 1
    else
        v_cash = v_cash + q * px * (1 - f)
        v_pos = v_pos - q
        flag = flag + 1
        sell_count = sell_count + 1
    end
    if flag > flag_max then
        flag_max = flag
    end
    if flag < flag_min then
        flag_min = flag
    end
    model_apply(fill, px, size)
    fill_count = fill_count + 1
    save_state(ctx)
    ctx:log(string.format(
        "[FILL] #%d %s 实际 %.6f @ %.4f (目标 %.6f, 费 %.4f) | flag=%d 虚拟现金 %.2f 虚拟持仓 %.6f " ..
        "真实持仓 %.6f 平衡价(新)=%.4f",
        fill_count, fill.side, size, px, q, fill.fee or 0, flag, v_cash, v_pos, m_pos, balance_price))
    return {}
end

-- 拒单回传: 建仓被拒 → 复位在途标记, 下个交叉重试; 目标量一并清零防串单。
function on_order_update(ctx, upd)
    local status = upd and upd.status
    if status == "rejected" then
        if pending_entry then
            pending_entry = false
        end
        pending_clip = 0
        ctx:log(string.format(
            "[shannon_virtual_grid] 订单被拒: pair=%s filled=%.6f remaining=%.6f",
            upd.pair, upd.filled_size or 0, upd.remaining_size or 0))
    end
end

function on_stop(ctx)
    local pair = ctx:config_str("pair")
    local pos = ctx:position_size(pair) or 0
    local cash = ctx:balance(quote_asset) or 0

    -- 真实账本交叉核对(回测规范 §C.4): 模型 vs 引擎, 误差 > 0.01 → WARN
    local d_cash = (m_cash or cash) - cash
    local d_pos = (m_pos or pos) - pos
    if math.abs(d_cash) > 0.01 or math.abs(d_pos) > 0.01 then
        ctx:log(string.format(
            "[shannon_virtual_grid] [WARN] 真实账本分叉: 模型现金 %.6f vs 引擎 %.6f (差 %.6f), " ..
            "模型持仓 %.8f vs 引擎 %.8f (差 %.8f)",
            m_cash or 0, cash, d_cash, m_pos or 0, pos, d_pos))
    end

    -- 虚拟账本 1:1 自洽核对: v_cash 应恒等于 v_pos × 平衡价(目标量推进的封闭恒等式)。
    local v_1to1 = 0
    if v_cash ~= nil and v_pos ~= nil and balance_price ~= nil and v_total > 0 then
        v_1to1 = (v_cash - v_pos * balance_price) / v_total
        if math.abs(v_1to1) > 0.01 then
            ctx:log(string.format(
                "[shannon_virtual_grid] [WARN] 虚拟账本 1:1 自洽核对失败: v_cash %.4f vs v_pos×平衡价 %.4f (差 %.6f)",
                v_cash, v_pos * balance_price, v_1to1))
        end
    end

    -- 虚拟-真实背离留档(合法分叉, 不告警): v_pos 与实际持仓的差值 = 被现金封顶的累计量。
    local divergence = (v_pos or 0) - pos

    -- 收益分解: 合计 = 期末权益 − 投入 = 持仓收益 + 交易收益(再平衡净贡献)
    local out = string.format(
        "[shannon_virtual_grid] 停机(**不清仓**): 成交 %d(买 %d 卖 %d) / 跳过(ATR未就绪 %d, 小名义 %d) / " ..
        "现金截断买 %d 次(累计名义 %.2f) / 重置 %d 次 / flag=%d(峰值 +%d/%d) / %s / 平衡价 %s / " ..
        "期末现金 %.2f 真实持仓 %.6f(市值 %.2f) 虚拟现金 %.2f 虚拟持仓 %.6f(背离 %.6f) 权益 %.2f / " ..
        "真实费 %.4f / 账本差(现金 %.6f 持仓 %.8f)",
        fill_count, buy_count, sell_count, skip_no_atr, skip_notional,
        cash_limited_buys, cash_limited_notional, reset_count,
        flag, flag_max, flag_min,
        halted and "已停机(参数/成本门槛)" or "正常运行",
        balance_price and string.format("%.4f", balance_price) or "nil",
        cash, pos, pos * (last_price or 0), v_cash or 0, v_pos or 0, divergence,
        ctx:equity() or 0, m_fee, d_cash, d_pos)
    if invested0 > 0 and last_price > 0 then
        local eq = ctx:equity() or 0
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

    -- stat_* 导出(报告"策略统计"区块)
    ctx:state_set("stat_eff_virtual_mult", string.format("%.4f", virtual_mult))
    ctx:state_set("stat_v_total", string.format("%.2f", v_total))
    ctx:state_set("stat_invested0", string.format("%.2f", invested0))
    ctx:state_set("stat_entry_price", string.format("%.6f", entry_price))
    ctx:state_set("stat_entry_size", string.format("%.6f", entry_size))
    ctx:state_set("stat_balance_price",
        balance_price and string.format("%.6f", balance_price) or "")
    ctx:state_set("stat_fill_count", string.format("%d", fill_count))
    ctx:state_set("stat_buy_count", string.format("%d", buy_count))
    ctx:state_set("stat_sell_count", string.format("%d", sell_count))
    ctx:state_set("stat_skip_no_atr", string.format("%d", skip_no_atr))
    ctx:state_set("stat_skip_notional", string.format("%d", skip_notional))
    ctx:state_set("stat_cash_limited_buys", string.format("%d", cash_limited_buys))
    ctx:state_set("stat_cash_limited_notional", string.format("%.2f", cash_limited_notional))
    ctx:state_set("stat_reset_count", string.format("%d", reset_count))
    ctx:state_set("stat_stall_bars", string.format("%d", stall_bars))
    ctx:state_set("stat_cash_final", string.format("%.2f", cash))
    ctx:state_set("stat_pos_final", string.format("%.8f", pos))
    ctx:state_set("stat_pos_value_final", string.format("%.2f", pos * (last_price or 0)))
    ctx:state_set("stat_v_cash_final", string.format("%.2f", v_cash or 0))
    ctx:state_set("stat_v_pos_final", string.format("%.8f", v_pos or 0))
    ctx:state_set("stat_v_pos_divergence", string.format("%.8f", divergence))
    ctx:state_set("stat_v_1to1_diff", string.format("%.8f", v_1to1))
    ctx:state_set("stat_fee_total", string.format("%.4f", m_fee))
    ctx:state_set("stat_ledger_diff_cash", string.format("%.6f", d_cash))
    ctx:state_set("stat_ledger_diff_pos", string.format("%.8f", d_pos))
    ctx:state_set("stat_halted", halted and "1" or "0")
    ctx:state_set("stat_flag", string.format("%d", flag))
    ctx:state_set("stat_flag_max", string.format("%d", flag_max))
    ctx:state_set("stat_flag_min", string.format("%d", flag_min))
    ctx:state_set("stat_eff_atr_interval", cfg_str(ctx, "atr_interval", "1m"))
    ctx:state_set("stat_eff_atr_period",
        string.format("%d", math.floor(num(ctx, "atr_period", 14))))
    ctx:state_set("stat_eff_atr_mult", string.format("%.4f", num(ctx, "atr_mult", 5)))
    ctx:state_set("stat_eff_min_spacing_pct", string.format("%.4f", num(ctx, "min_spacing_pct", 0.004)))
    ctx:state_set("stat_eff_flag_spacing_mult", string.format("%.4f", num(ctx, "flag_spacing_mult", 1.2)))
    ctx:state_set("stat_eff_ema_interval", cfg_str(ctx, "ema_interval", "1m"))
    ctx:state_set("stat_eff_ema_fast", string.format("%d", math.floor(num(ctx, "ema_fast", 2))))
    ctx:state_set("stat_eff_ema_slow", string.format("%d", math.floor(num(ctx, "ema_slow", 3))))
    ctx:state_set("stat_eff_min_notional", string.format("%.2f", num(ctx, "min_notional", 5)))
    ctx:state_set("stat_eff_fee_side", string.format("%.4f", num(ctx, "fee_side", 0.001)))
end
