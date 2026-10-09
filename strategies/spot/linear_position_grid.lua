-- 现货线性仓位网格 (linear_position_grid)
--
-- ═══ 一句话 ═══
--   在 [p_low, p_high] 区间内, 仓位占比 = 币市值 ÷ 总权益(币市值+现金) 随价格**线性分布**(低价高仓位、高价低仓位);
--   分三阶段运行: 逐步建仓 → 线性网格(成交即全撤、在成交价上下 1×ATR 两侧挂限价单) → 可选动态止盈。
--   反复买卖把波动差价转化为现金收益。
--
-- ═══ 策略含义(逐条) ═══
--   1) 线性仓位(用户口径 2026-10-09, 唯一定义): 仓位 = Q×p / (Q×p + 现金)。
--      w(p) = pos_low_pct + (pos_high_pct − pos_low_pct)×(p − p_low)/(p_high − p_low), 钳制 [pos_high_pct, pos_low_pct]。
--      **每次买卖成交后, 按成交价 p 精确满足 Q×p/(Q×p+现金) = w(p) —— 什么价格就是什么仓位。**
--      含费修正(成交后权益 E′ = E − 费): 买 q = (w·E − Q·p) / (p·(1+w·f)); 卖 q = (Q·p − w·E) / (p·(1−w·f)),
--      其中 E = Q×p + 现金(按腿价估值的当前总权益), f = fee_side。w<1 时现金恒 = (1−w)×E > 0, 永不被买干。
--   2) 逐步建仓(独立阶段): 目标总量 = T(start_price) 均分 build_steps 份币量; 价格**低于 start_price** 且
--      距上次建仓 ≥ build_interval_hours 小时才市价买入一份(高价到点顺延, 不烧步数)。建仓期间不挂任何网格单。
--      建满 build_steps 份 → 进入网格, 参考价 ref := 最后一笔建仓成交价, 最高价 peak := ref。
--   3) 网格: ref − Δ 挂买单、ref + Δ 挂卖单(Δ = atr_mult×ATR, 下限 min_spacing_pct×价格; 买卖价钳在区间内)。
--      买量/卖量按第 1 条落位公式在**腿价**上计算 → 该腿成交后仓位精确 = w(成交价)。
--   4) 成交(全撤重挂, 事件模型): 任一腿成交 → ref := 成交价 → 撤光两侧 → 在新 ref 上下重挂。
--      链内新限价单由引擎次 bar 生效(防乒乓链)。现价偏离 ref 超过一个间距时同步重锚 ref := 现价
--      并重挂(防单边/跳空行情下另一腿被穿越守卫长期抑制 → 死锁零成交)。
--   5) 实时估值: 落位目标恒按"当前总权益×w"计算, 交易赚取的差价即时进入现金侧, 直接参与后续仓位计算 —— 无沉淀资金。
--   6) 出界处理 out_of_range: "exit"(默认)= 价格出区间即撤光挂单停机、**不清仓**;"wait"= 暂停挂单, 等价格
--      回到区间再恢复(ref 重锚现价)。
--   7) 动态止盈(可选, tp_min_profit_pct=0 关闭): 网格阶段追踪价格最高值 peak; 当 peak − 现价 ≥
--      tp_dd_atr_mult×ATR **且** (权益 − C0)/C0 ≥ tp_min_profit_pct 时, 撤光 + 市价清仓 + 停机。
--      ⚠ 启用后单边上涨行情会在首次回撤即清仓离场, 可能"提前下车"错过后续涨幅。
--
-- ═══ 参数(全部可配, 默认值可直接回测) ═══
--   pair                 必填, 现货交易对
--   start_price          必填, 开始价格(低于它才逐步建仓)
--   p_low / p_high       必填, 线性仓位价格区间(须满足 p_low < start_price < p_high)
--   pos_low_pct          p_low 处仓位占比, 默认 0.7
--   pos_high_pct         p_high 处仓位占比, 默认 0.3(直读, 0 = 顶部清仓合法)
--   invest_cash          投入资金 C0, 默认 0(= 首次建仓时 quote 余额全额)
--   build_steps          分几批建仓, 默认 10
--   build_interval_hours 每批最小间隔(小时), 默认 1
--   interval             主时钟, 默认 1h(回测建议 1m)
--   atr_interval         算 ATR 的 K 线周期, 默认 1h
--   atr_period           ATR 周期, 默认 14
--   atr_mult             网格间距 = atr_mult × ATR, 默认 1
--   min_spacing_pct      间距下限(占价格比例), 默认 0.004; 负数 = 禁用下限
--   min_notional         单笔最小名义, 默认 5
--   fee_side             单边费率, 默认 0.001
--   out_of_range         出界处理, "exit"(默认)/"wait"
--   tp_min_profit_pct    动态止盈最小盈利比例, 默认 0.10(直读, 0 = 禁用)
--   tp_dd_atr_mult       动态止盈最高点回撤 ATR 倍数, 默认 3
--
-- ═══ 守卫与可观测性 ═══
--   参数越界 → [FATAL] 停机; ATR 未就绪 → 不挂单不猜值(计数); 买量现金兜底(预扣费+滑点); 卖量持仓兜底(1e-9);
--   挂腿穿越市价 → 抑制该腿(防恢复/钳边时被动扫仓); 成本门槛: 生效间距 < 4×fee_side×价格 → [FATAL];
--   连续 ≥1 天无任何挂单 → WARN(停摆); 策略账本(m_cash/m_pos)停机时与引擎交叉核对(误差 > 0.01 → WARN);
--   每次成交后落位误差追踪(stat_pos_err_max)。
--
-- ═══ 用法 ═══
--   ricow backtest --strategy linear_position_grid --pair SOLUSDT --interval 1m --days 365 --cash 10000 \
--     --param start_price=<窗口起点价> --param p_low=<起点×0.7> --param p_high=<起点×1.4> \
--     --param invest_cash=10000 --param build_steps=10 --param build_interval_hours=1
--   TOML: type = "linear_position_grid" + [strategy.params] 填上面参数表。
--
quote_asset = ""  -- 计价资产, on_init 里 detect_quote(pair) 动态检测

-- 线性仓位与投入
C0 = 0                    -- 投入资金(收益基准; invest_cash>0 启动定格, =0 首次建仓定格)
P_low = 0
P_high = 0
W_LO = 0.7                -- p_low 处仓位占比
W_HI = 0.3                -- p_high 处仓位占比
build_target = 0          -- 建仓目标总币量 = T(start_price)
step_size = 0             -- 每批建仓币量 = build_target / build_steps

-- 阶段与网格状态
phase = "build"           -- "build" | "grid"
built = false             -- 建仓完成进入网格
pending_step = false      -- 建仓市价单在途
ref_price = nil           -- 参考价(最近成交价)
peak_price = nil          -- 网格阶段价格最高值(动态止盈基准)
need_rehang = false       -- 成交/续接/恢复/偏离后置 true: 下一 tick 撤旧单 + 重挂
paused = false            -- out_of_range=wait 出界暂停
halted = false            -- 停机(出界 exit / 止盈 / 参数 / 成本门槛)
tp_closed = false         -- 动态止盈已清仓
fatal = 0                 -- 数值化停机标记(单测经 global_f64 读取)
cost_checked = false
last_bar_ts = nil
last_step_ts = nil        -- 上次建仓时刻(秒)
last_price = 0

-- 账本模型(独立记账, 与引擎对账)
m_cash = nil
m_pos = nil
m_fee = 0
entry_price = 0           -- 首笔建仓成交价
entry_size = 0
build_filled_qty = 0      -- 建仓累计成交币量
build_cost = 0            -- 建仓累计花费(含费)
pos_err_max = 0           -- 落位不变式最大误差(每次成交后 |实际仓位 − w(成交价)|)
pos_err_px = 0            -- 误差最大时的成交价

-- 计数(可观测性)
fill_count = 0
buy_count = 0
sell_count = 0
build_done_steps = 0      -- 已完成建仓批数(成交计)
rehang_count = 0
skip_no_atr = 0
skip_notional = 0
stall_bars = 0
stall_bars_max = 0
stall_since = nil
stall_warned_at = nil

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

-- 线性仓位占比 w(p), 钳制在 [W_HI, W_LO]
local function w_of(p)
    if p <= P_low then
        return W_LO
    end
    if p >= P_high then
        return W_HI
    end
    local w = W_LO + (W_HI - W_LO) * (p - P_low) / (P_high - P_low)
    return w
end

-- 目标仓位占比 w(p) 下的成交后落位公式(用户口径: 仓位 = 币市值 ÷ 总权益):
-- 按腿价 p 估值当前总权益 E = Q×p + C; 成交后精确满足 Q'×p / (Q'×p + C') = w(p)(含费修正):
--   买 q = (w·E − Q·p) / (p·(1 + w·f));  卖 q = (Q·p − w·E) / (p·(1 − w·f))
-- w<1 时成交后现金恒 = (1−w)×E > 0 → 价格不到 p_low 现金绝不耗尽。
local function buy_qty(w, Q, C, p, f)
    local E = Q * p + C
    return (w * E - Q * p) / (p * (1 + w * f))
end

local function sell_qty(w, Q, C, p, f)
    local E = Q * p + C
    return (Q * p - w * E) / (p * (1 - w * f))
end

-- 建仓目标币量: 初始全现金(E=C0), 建仓后 Q×start = w(start)×C0 → Q = w(start)×C0/start。
local function T_of(p)
    if p <= 0 or C0 <= 0 then
        return 0
    end
    return w_of(p) * C0 / p
end

-- 买量现金兜底(预扣手续费 + 滑点余量)
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

-- 生效间距 Δ = max(atr_mult×ATR, min_spacing_pct×价格); ATR 未就绪返回 nil。
-- atr_interval = 主时钟走 ctx:atr(primary 尾窗), 跨周期走 ctx:atr_tf(TfCache) —— 防 O(n²)。
local function spacing_of(ctx, pair, price)
    local atr_intv = cfg_str(ctx, "atr_interval", "1h")
    local main_intv = cfg_str(ctx, "interval", "1h")
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
        return nil, nil
    end
    local spacing = num(ctx, "atr_mult", 1) * atr
    local min_spacing = num(ctx, "min_spacing_pct", 0.004) * price
    if min_spacing < 0 then
        min_spacing = 0
    end
    if spacing < min_spacing then
        spacing = min_spacing
    end
    return spacing, atr
end

-- 模型账本按真实成交推进(fill.fee 已含, 引擎同源口径)
local function model_apply(fill, px, size)
    if m_cash == nil then
        return
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
    if ref_price ~= nil then
        ctx:state_set("ref_price", string.format("%.10f", ref_price))
    end
    if peak_price ~= nil then
        ctx:state_set("peak_price", string.format("%.10f", peak_price))
    end
    ctx:state_set("phase", phase)
    ctx:state_set("built", built and "1" or "0")
    ctx:state_set("build_done_steps", string.format("%d", build_done_steps))
    ctx:state_set("halted", halted and "1" or "0")
    ctx:state_set("tp_closed", tp_closed and "1" or "0")
    ctx:state_set("paused", paused and "1" or "0")
    ctx:state_set("pending_step", pending_step and "1" or "0")
    if last_step_ts ~= nil then
        ctx:state_set("last_step_ts", string.format("%d", last_step_ts))
    end
    if C0 > 0 then
        ctx:state_set("C0", string.format("%.10f", C0))
    end
end

function on_init(ctx)
    local pair = ctx:config_str("pair")
    quote_asset = exec.detect_quote(pair)
    -- 数据需求声明: 主时钟 + ATR 序列。
    local main_intv = cfg_str(ctx, "interval", "1h")
    ctx:need_klines("primary", main_intv, 1000)
    local atr_intv = cfg_str(ctx, "atr_interval", "1h")
    local atr_need = math.floor(num(ctx, "atr_period", 14)) + 1
    if atr_need < 24 then
        atr_need = 24
    end
    ctx:need_klines("aux", atr_intv, atr_need)

    P_low = ctx:config_f64("p_low") or 0
    P_high = ctx:config_f64("p_high") or 0
    W_LO = num(ctx, "pos_low_pct", 0.7)
    -- pos_high_pct 直读(0 合法 = 顶部清仓, 不能用 num 兜底); 缺省值由清单 default 注入生效配置。
    W_HI = ctx:config_f64("pos_high_pct")
    local start_px = ctx:config_f64("start_price") or 0
    local build_steps = math.floor(num(ctx, "build_steps", 10))
    local build_intv = num(ctx, "build_interval_hours", 1)

    -- 参数校验(纯 config, 幂等): 越界即 FATAL 停机。
    local bad
    if P_low <= 0 or P_high <= 0 or P_low >= P_high then
        bad = string.format("价格区间非法: p_low=%.6f p_high=%.6f (须 0 < p_low < p_high)", P_low, P_high)
    elseif start_px <= 0 or start_px <= P_low or start_px >= P_high then
        bad = string.format("start_price=%.6f 须落在区间内 (p_low=%.6f < start_price < p_high=%.6f)", start_px, P_low, P_high)
    elseif W_HI < 0 or W_LO > 1 or W_HI >= W_LO then
        bad = string.format("仓位占比非法: pos_high_pct=%.4f pos_low_pct=%.4f (须 0 ≤ pos_high_pct < pos_low_pct ≤ 1)", W_HI, W_LO)
    elseif build_steps < 1 then
        bad = string.format("build_steps=%d 须 ≥ 1", build_steps)
    elseif build_intv <= 0 then
        bad = string.format("build_interval_hours=%.4f 须 > 0", build_intv)
    end
    if bad then
        halted = true
        fatal = 1
        ctx:log("[linear_grid] [FATAL] 参数校验失败 -> 停机: " .. bad)
        return
    end

    -- C0 定格: invest_cash>0 启动即定; =0 延迟到首次建仓(那时余额才就绪)。
    C0 = ctx:config_f64("invest_cash") or 0
    if C0 > 0 then
        build_target = T_of(start_px)
        step_size = build_target / build_steps
    end

    phase = "build"
    built = false
    pending_step = false
    ref_price = nil
    peak_price = nil
    need_rehang = false
    paused = false
    halted = false
    tp_closed = false
    cost_checked = false
    m_cash = nil
    m_pos = nil
    m_fee = 0

    -- 断点续接
    local rp = ctx:state_get("ref_price")
    if rp ~= nil and tonumber(rp) and tonumber(rp) > 0 then
        ref_price = tonumber(rp)
        local pk = ctx:state_get("peak_price")
        peak_price = (pk and tonumber(pk)) or ref_price
        phase = ctx:state_get("phase") or "grid"
        built = (ctx:state_get("built") == "1")
        build_done_steps = tonumber(ctx:state_get("build_done_steps")) or 0
        pending_step = (ctx:state_get("pending_step") == "1")
        local lst = ctx:state_get("last_step_ts")
        if lst and tonumber(lst) then
            last_step_ts = tonumber(lst)
        end
        local c0s = ctx:state_get("C0")
        if c0s and tonumber(c0s) then
            C0 = tonumber(c0s)
            build_target = T_of(start_px)
            step_size = build_target / build_steps
        end
        m_cash = ctx:balance(quote_asset) or 0
        m_pos = ctx:position_size(pair) or 0
        halted = (ctx:state_get("halted") == "1")
        tp_closed = (ctx:state_get("tp_closed") == "1")
        paused = (ctx:state_get("paused") == "1")
        if halted or tp_closed then
            ctx:log(string.format(
                "[linear_grid] 续接: 上次已停机(halted=%s, 止盈清仓=%s) -> 本次不再交易",
                tostring(halted), tostring(tp_closed)))
        elseif built then
            need_rehang = true
            ctx:log(string.format("[linear_grid] 续接: 网格阶段 ref=%.4f peak=%.4f", ref_price, peak_price))
        else
            ctx:log(string.format("[linear_grid] 续接: 建仓阶段 已完成 %d 批", build_done_steps))
        end
    end

    ctx:log(string.format(
        "[linear_grid] init pair=%s quote=%s 区间[%.4f,%.4f] 仓位[%.2f→%.2f] start=%.4f C0=%s " ..
        "建仓=%d批×%.4fh ATR=%s×%d mult=%.2f out_of_range=%s 止盈=%s/%.1f×ATR",
        pair, quote_asset, P_low, P_high, W_LO, W_HI, start_px,
        C0 > 0 and string.format("%.2f", C0) or "首次建仓时定格",
        build_steps, build_intv, atr_intv, math.floor(num(ctx, "atr_period", 14)),
        num(ctx, "atr_mult", 1), cfg_str(ctx, "out_of_range", "exit"),
        (ctx:config_f64("tp_min_profit_pct") or 0.10) > 0 and string.format("%.1f%%", (ctx:config_f64("tp_min_profit_pct") or 0.10) * 100) or "禁用",
        num(ctx, "tp_dd_atr_mult", 3)))
end

-- 全撤重挂(共享决策出口, 事件模型): 撤旧单 + 下方买单 + 上方卖单。
-- 量按落位公式在腿价上计算; 买卖价钳在区间内; 穿越市价的腿抑制(防被动扫仓);
-- 现价偏离 ref 超一个间距时先重锚 ref := 现价(防单边/跳空死锁)。
function do_rehang(ctx)
    local pair = ctx:config_str("pair")
    local price = ctx:price(pair)
    if not price or price <= 0 then
        return {}
    end
    local min_notional = num(ctx, "min_notional", 5)
    local fee_side = num(ctx, "fee_side", 0.001)
    local fee_bps = fee_side * 10000

    local spacing, atr = spacing_of(ctx, pair, price)
    if not spacing then
        skip_no_atr = skip_no_atr + 1
        stall_bars = stall_bars + 1
        if stall_bars > stall_bars_max then
            stall_bars_max = stall_bars
        end
        if stall_since == nil then
            stall_since = (ctx:now() or {}).ts
        end
        return {}
    end

    -- 成本门槛(启动硬校验一次): 生效间距必须 ≥ 4×单边费率。
    if not cost_checked then
        cost_checked = true
        local need = 4 * fee_side * price
        if spacing < need then
            halted = true
            fatal = 1
            save_state(ctx)
            ctx:log(string.format(
                "[linear_grid] [FATAL] 成本门槛不满足 -> 停机: 生效间距=%.6f(%.4f%%) < 4×fee_side×价格=%.6f(%.4f%%)",
                spacing, spacing / price * 100, need, need / price * 100))
            return {}
        end
    end

    local C = ctx:balance(quote_asset) or 0
    local Q = ctx:position_size(pair) or 0
    local f = fee_side

    -- 重锚(防单边死锁): 现价偏离 ref 超过一个间距 → ref := 现价。跳空成交或单边行情会让
    -- ref 落到市场一侧、另一腿被穿越守卫抑制 → 无成交事件则永不重挂 → 死锁(整月零成交)。
    -- 重锚后两侧腿重新分列市场两侧, 且仓位再平衡回 w(现价)(= 用户口径"什么价格什么仓位")。
    if math.abs(price - ref_price) > spacing then
        ref_price = price
        if peak_price == nil or price > peak_price then
            peak_price = price
        end
    end

    local buy_px = ref_price - spacing
    if buy_px < P_low then
        buy_px = P_low
    end
    local sell_px = ref_price + spacing
    if sell_px > P_high then
        sell_px = P_high
    end

    local orders = { { pair = pair, action = "cancel_pending" } }

    -- 买单: 价须低于现价(否则开盘即成交 → 被动扫货)。
    -- 成交后按成交价精确落位: Q'×buy_px/(Q'×buy_px + C') = w(buy_px)。
    -- w<1 时花费恒 < 现金(数学保证), cap_buy 仅护 w→1 极端边界, 常规区间不触发。
    if buy_px > 0 and buy_px < price then
        local q = buy_qty(w_of(buy_px), Q, C, buy_px, f)
        if q > 0 then
            q = cap_buy(ctx, q, buy_px, fee_bps)
            if q > 0 and q * buy_px >= min_notional then
                orders[#orders + 1] = {
                    pair = pair, side = "buy", size = q, order_type = "limit", price = buy_px,
                }
            end
        end
    end

    -- 卖单: 价须高于现价; 成交后按成交价精确落位 w(sell_px)。q ≤ Q 恒成立, cap_sell 仅防 f64 舍入。
    if Q > 0 and sell_px > price then
        local q = sell_qty(w_of(sell_px), Q, C, sell_px, f)
        if q > 0 then
            q = cap_sell(ctx, pair, q)
            if q > 0 and q * sell_px >= min_notional then
                orders[#orders + 1] = {
                    pair = pair, side = "sell", size = q, order_type = "limit", price = sell_px,
                }
            end
        end
    end

    if #orders <= 1 then
        skip_notional = skip_notional + 1
        stall_bars = stall_bars + 1
        if stall_since == nil then
            stall_since = (ctx:now() or {}).ts
        end
        local now = (ctx:now() or {}).ts
        if now ~= nil and stall_since ~= nil and now - stall_since >= 86400
            and (stall_warned_at == nil or now - stall_warned_at >= 86400) then
            stall_warned_at = now
            ctx:log(string.format(
                "[linear_grid] [WARN] 停摆 ≥1 天: ref %.4f 间距 %.4f 两侧均无法挂单(现金 %.2f 持仓 %.6f)",
                ref_price, spacing, C, Q))
        end
        return {}
    end

    stall_bars = 0
    stall_since = nil
    stall_warned_at = nil
    need_rehang = false
    rehang_count = rehang_count + 1
    save_state(ctx)
    local has_buy, has_sell = false, false
    for i = 2, #orders do
        if orders[i].side == "buy" then
            has_buy = true
        else
            has_sell = true
        end
    end
    ctx:log(string.format(
        "[linear_grid] 网格重挂 #%d: ref %.4f 间距 %.4f (ATR %s) 区间[%.4f,%.4f] -> 买 %s / 卖 %s | 现金 %.2f 持仓 %.6f",
        rehang_count, ref_price, spacing, atr and string.format("%.4f", atr) or "nil", P_low, P_high,
        has_buy and string.format("%.4f", buy_px) or "无",
        has_sell and string.format("%.4f", sell_px) or "无",
        C, Q))
    return orders
end

function on_tick(ctx)
    if halted or tp_closed then
        return {}
    end
    local pair = ctx:config_str("pair")

    -- 决策节流: 只在主时钟新 bar 决策一次。
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
    last_bar_ts = t.ts

    local start_px = ctx:config_f64("start_price") or 0
    local min_notional = num(ctx, "min_notional", 5)
    local fee_side = num(ctx, "fee_side", 0.001)
    local fee_bps = fee_side * 10000
    local build_steps = math.floor(num(ctx, "build_steps", 10))
    local build_intv = num(ctx, "build_interval_hours", 1)

    -- ── 出界处理(建仓/网格两阶段通用: 建仓期无挂单, exit 即停 / wait 即暂停)
    local mode = cfg_str(ctx, "out_of_range", "exit")
    if price < P_low or price > P_high then
        if mode == "exit" then
            halted = true
            save_state(ctx)
            ctx:log(string.format(
                "[linear_grid] 价格 %.4f 出区间 [%.4f,%.4f] -> exit 停机(**不清仓**), 撤光挂单",
                price, P_low, P_high))
            return { { pair = pair, action = "cancel_pending" } }
        else
            -- wait: 暂停挂单, 等回界内
            if not paused then
                paused = true
                save_state(ctx)
                ctx:log(string.format(
                    "[linear_grid] 价格 %.4f 出区间 -> wait 暂停(等回界内)", price))
                return { { pair = pair, action = "cancel_pending" } }
            end
            return {}
        end
    elseif paused then
        -- 回界内: 网格阶段 ref 重锚现价恢复挂单; 建仓阶段直接恢复建仓节奏
        paused = false
        if built then
            ref_price = price
            need_rehang = true
        end
        save_state(ctx)
        ctx:log(string.format("[linear_grid] 价格 %.4f 回区间 -> 恢复(%s)", price, built and "ref 重锚现价" or "建仓"))
    end

    -- ── 建仓阶段(独立过程, 不挂网格单)
    if not built then
        if pending_step then
            return {} -- 建仓市价单在途, 等成交回调
        end
        -- C0 延迟定格(invest_cash=0)
        if C0 <= 0 then
            C0 = ctx:balance(quote_asset) or 0
            if C0 <= 0 then
                return {}
            end
            build_target = T_of(start_px)
            step_size = build_target / build_steps
        end
        -- 仅价格低于 start_price 才建仓; 距上次建仓须 ≥ build_intv 小时(高价到点顺延)。
        if price >= start_px then
            return {}
        end
        if last_step_ts ~= nil and (t.ts - last_step_ts) < build_intv * 3600 then
            return {}
        end
        local size = cap_buy(ctx, step_size, price, fee_bps)
        if size > 0 and size * price >= min_notional then
            pending_step = true
            last_step_ts = t.ts
            save_state(ctx)
            ctx:log(string.format(
                "[linear_grid] 建仓第 %d/%d 批: 价 %.4f < start %.4f -> 市价买入 %.6f(≈%.2f %s)",
                build_done_steps + 1, build_steps, price, start_px, size, size * price, quote_asset))
            return { { pair = pair, side = "buy", size = size, order_type = "market" } }
        end
        -- 名义过小: 该批跳过(不烧步数, 下 tick 再试)。
        skip_notional = skip_notional + 1
        return {}
    end

    -- ── 网格阶段
    if ref_price == nil then
        return {}
    end

    -- 动态止盈(仅网格阶段; tp_min_profit_pct=0 关闭, 默认值由清单注入)
    local tp_min = ctx:config_f64("tp_min_profit_pct")
    if tp_min > 0 then
        if peak_price == nil or price > peak_price then
            peak_price = price
        end
        local spacing, atr = spacing_of(ctx, pair, price)
        if spacing and atr then
            local dd = peak_price - price
            local dd_need = num(ctx, "tp_dd_atr_mult", 3) * atr
            local eq = ctx:equity() or 0
            local profit = (eq - C0) / C0
            if dd_need > 0 and dd >= dd_need and profit >= tp_min then
                tp_closed = true
                halted = true
                save_state(ctx)
                local pos = ctx:position_size(pair) or 0
                ctx:log(string.format(
                    "[linear_grid] 动态止盈触发: peak %.4f 现价 %.4f 回撤 %.4f ≥ %.1f×ATR(%.4f) " ..
                    "且盈利 %.2f%% ≥ %.2f%% -> 撤光+市价清仓(%.6f)+停机",
                    peak_price, price, dd, num(ctx, "tp_dd_atr_mult", 3), atr, profit * 100, tp_min * 100, pos))
                local sell_size = cap_sell(ctx, pair, pos)
                local out = { { pair = pair, action = "cancel_pending" } }
                if sell_size > 0 and sell_size * price >= min_notional then
                    out[#out + 1] = { pair = pair, side = "sell", size = sell_size, order_type = "market" }
                end
                return out
            end
        end
    end

    -- 偏离触发重挂(防单边死锁): 现价偏离 ref 超过一个间距 → 置 need_rehang。
    local spacing_now = spacing_of(ctx, pair, price)
    if spacing_now and ref_price and math.abs(price - ref_price) > spacing_now then
        need_rehang = true
    end

    if need_rehang then
        return do_rehang(ctx)
    end
    return {}
end

function on_fill(ctx, fill)
    local pair = ctx:config_str("pair")
    local px = fill.fill_price or 0
    local size = fill.fill_size or 0

    -- 停机/止盈后的成交(止盈市价单本身、halt 同 bar 已撮合的腿): 只入账, 不再产单 ——
    -- 保证账本与引擎逐笔同步(规范 §C.4)。
    if halted or tp_closed then
        model_apply(fill, px, size)
        if fill.side == "buy" then
            buy_count = buy_count + 1
        else
            sell_count = sell_count + 1
        end
        fill_count = fill_count + 1
        return {}
    end

    if pending_step then
        -- 建仓成交
        pending_step = false
        build_done_steps = build_done_steps + 1
        build_filled_qty = build_filled_qty + size
        build_cost = build_cost + size * px + (fill.fee or 0)
        if entry_price == 0 then
            entry_price = px
            entry_size = size
        end
        ref_price = px
        if m_cash == nil then
            m_cash = ctx:balance(quote_asset) or 0
            m_pos = ctx:position_size(pair) or 0
            m_fee = fill.fee or 0
        else
            model_apply(fill, px, size)
        end
        buy_count = buy_count + 1
        fill_count = fill_count + 1
        local build_steps = math.floor(num(ctx, "build_steps", 10))
        if build_done_steps >= build_steps then
            built = true
            phase = "grid"
            peak_price = px
            need_rehang = true
            ctx:log(string.format(
                "[linear_grid] 建仓完成 %d 批(共 %.6f, 均价成本 %.2f) -> 进入网格, ref=peak := %.4f",
                build_done_steps, build_filled_qty, build_cost, ref_price))
            save_state(ctx)
            return do_rehang(ctx)
        end
        ctx:log(string.format(
            "[linear_grid] 建仓成交 第 %d/%d 批 %.6f @ %.4f -> ref := %.4f",
            build_done_steps, build_steps, size, px, ref_price))
        save_state(ctx)
        return {}
    end

    -- 网格成交: ref := 成交价; peak 只升不降; 落位误差核验; 全撤重挂
    ref_price = px
    if peak_price == nil or px > peak_price then
        peak_price = px
    end
    model_apply(fill, px, size)
    if fill.side == "buy" then
        buy_count = buy_count + 1
    else
        sell_count = sell_count + 1
    end
    fill_count = fill_count + 1
    -- 落位不变式核验(用户口径): 成交后按成交价 px 实算仓位 = Q·px/(Q·px+现金), 应 = w(px)。
    local Q_now = ctx:position_size(pair) or 0
    local C_now = ctx:balance(quote_asset) or 0
    local denom = Q_now * px + C_now
    local w_act = (denom > 0) and (Q_now * px / denom) or 0
    local err = math.abs(w_act - w_of(px))
    if err > pos_err_max then
        pos_err_max = err
        pos_err_px = px
    end
    need_rehang = true
    save_state(ctx)
    ctx:log(string.format(
        "[FILL] #%d %s size=%.6f px=%.4f notional=%.2f 费=%.4f | 现金 %.2f 持仓 %.6f " ..
        "仓位=%.4f%% 目标 w=%.4f%% (差 %.6f) ref(新)=%.4f peak=%.4f",
        fill_count, fill.side, size, px, size * px, fill.fee or 0,
        C_now, Q_now, w_act * 100, w_of(px) * 100, err, ref_price, peak_price or 0))
    return do_rehang(ctx)
end

-- 拒单回传: 只处理 rejected(cancelled 是主动撤单的正常回传, 触发重挂会死循环)。
function on_order_update(ctx, upd)
    local status = upd and upd.status
    if status == "rejected" then
        if pending_step then
            pending_step = false -- 建仓被拒: 恢复, 下个 tick 重试该批
            last_step_ts = nil
        end
        need_rehang = true
        ctx:log(string.format(
            "[linear_grid] 挂单被拒: pair=%s filled=%.6f remaining=%.6f → 重挂",
            upd.pair, upd.filled_size or 0, upd.remaining_size or 0))
    end
end

function on_stop(ctx)
    local pair = ctx:config_str("pair")
    local pos = ctx:position_size(pair) or 0
    local cash = ctx:balance(quote_asset) or 0
    local eq = ctx:equity() or 0

    -- 账本交叉核对
    local d_cash = (m_cash or cash) - cash
    local d_pos = (m_pos or pos) - pos
    if math.abs(d_cash) > 0.01 or math.abs(d_pos) > 0.01 then
        ctx:log(string.format(
            "[linear_grid] [WARN] 账本分叉: 模型现金 %.6f vs 引擎 %.6f (差 %.6f), 模型持仓 %.8f vs 引擎 %.8f (差 %.8f)",
            m_cash or 0, cash, d_cash, m_pos or 0, pos, d_pos))
    end

    -- 收益分解: 合计 = 期末权益 − C0 = 持仓收益 + 交易(网格)收益
    local out = string.format(
        "[linear_grid] 停机(%s): 阶段=%s 成交 %d(买 %d 卖 %d) 建仓 %d 批 / 重挂 %d / 跳过(ATR %d 名义 %d) / " ..
        "ref %s peak %s / 期末现金 %.2f 持仓 %.6f(市值 %.2f) 权益 %.2f / 模型费 %.4f / 账本差(现金 %.6f 持仓 %.8f)",
        tp_closed and "动态止盈清仓" or (halted and "停机" or "正常运行"),
        phase, fill_count, buy_count, sell_count, build_done_steps, rehang_count,
        skip_no_atr, skip_notional,
        ref_price and string.format("%.4f", ref_price) or "nil",
        peak_price and string.format("%.4f", peak_price) or "nil",
        cash, pos, pos * (last_price or 0), eq, m_fee, d_cash, d_pos)
    if C0 > 0 and last_price > 0 then
        local total = eq - C0
        local hold = 0
        if entry_price > 0 then
            hold = pos * (last_price - entry_price)
        end
        -- 仓位口径 = 用户定义: 币市值 ÷ 总权益(= Q×末价/(Q×末价+现金)); 目标 = w(末价)。
        local denom = pos * last_price + cash
        local w_act = (denom > 0) and (pos * last_price / denom) or 0
        out = out .. string.format(
            " | 收益分解: 投入 %.2f | 合计 %+.2f = 持仓 %+.2f + 交易(网格) %+.2f | 期末仓位=%.4f%% 目标 w=%.4f%% | 落位最大误差 %.6f%s",
            C0, total, hold, total - hold,
            w_act * 100, w_of(last_price) * 100, pos_err_max,
            pos_err_max > 0.001 and string.format("(成交价 %.4f)", pos_err_px) or "")
        ctx:state_set("stat_w_actual", string.format("%.6f", w_act))
        ctx:state_set("stat_w_target", string.format("%.6f", w_of(last_price)))
        ctx:state_set("stat_pos_err_max", string.format("%.8f", pos_err_max))
    end
    ctx:log(out)

    -- stat_* 导出
    ctx:state_set("stat_invested0", string.format("%.2f", C0))
    ctx:state_set("stat_build_target", string.format("%.6f", build_target))
    ctx:state_set("stat_build_steps", string.format("%d", build_done_steps))
    ctx:state_set("stat_entry_price", string.format("%.6f", entry_price))
    ctx:state_set("stat_ref_price", ref_price and string.format("%.6f", ref_price) or "")
    ctx:state_set("stat_peak_price", peak_price and string.format("%.6f", peak_price) or "")
    ctx:state_set("stat_fill_count", string.format("%d", fill_count))
    ctx:state_set("stat_buy_count", string.format("%d", buy_count))
    ctx:state_set("stat_sell_count", string.format("%d", sell_count))
    ctx:state_set("stat_rehang_count", string.format("%d", rehang_count))
    ctx:state_set("stat_skip_no_atr", string.format("%d", skip_no_atr))
    ctx:state_set("stat_skip_notional", string.format("%d", skip_notional))
    ctx:state_set("stat_stall_bars", string.format("%d", stall_bars_max))
    ctx:state_set("stat_cash_final", string.format("%.2f", cash))
    ctx:state_set("stat_pos_final", string.format("%.8f", pos))
    ctx:state_set("stat_pos_value_final", string.format("%.2f", pos * (last_price or 0)))
    ctx:state_set("stat_fee_total", string.format("%.4f", m_fee))
    ctx:state_set("stat_ledger_diff_cash", string.format("%.6f", d_cash))
    ctx:state_set("stat_ledger_diff_pos", string.format("%.8f", d_pos))
    ctx:state_set("stat_halted", halted and "1" or "0")
    ctx:state_set("stat_tp_closed", tp_closed and "1" or "0")
    ctx:state_set("stat_paused", paused and "1" or "0")
    ctx:state_set("stat_phase", phase)
    ctx:state_set("stat_eff_atr_interval", cfg_str(ctx, "atr_interval", "1h"))
    ctx:state_set("stat_eff_atr_period", string.format("%d", math.floor(num(ctx, "atr_period", 14))))
    ctx:state_set("stat_eff_atr_mult", string.format("%.4f", num(ctx, "atr_mult", 1)))
    ctx:state_set("stat_eff_min_spacing_pct", string.format("%.4f", num(ctx, "min_spacing_pct", 0.004)))
    ctx:state_set("stat_eff_min_notional", string.format("%.2f", num(ctx, "min_notional", 5)))
    ctx:state_set("stat_eff_fee_side", string.format("%.4f", num(ctx, "fee_side", 0.001)))
    ctx:state_set("stat_eff_out_of_range", cfg_str(ctx, "out_of_range", "exit"))
    ctx:state_set("stat_eff_tp_min_profit_pct", string.format("%.4f", ctx:config_f64("tp_min_profit_pct") or 0.10))
    ctx:state_set("stat_eff_tp_dd_atr_mult", string.format("%.4f", num(ctx, "tp_dd_atr_mult", 3)))
end
