-- 合约香农网格 (shannon_grid_futures)
--
-- ═══ 语义 ═══
--   USDT-M 合约(hedge 模式, 只下 LONG 侧)香农网格: 价格低于 start_price 激活, 用总资金(投入×杠杆)
--   的一半市价开多建仓; 虚拟账本(现金 v_cash + 仓位 v_pos)恒 1:1 价值平衡, 平衡价 − 买间距 / + 卖间距
--   挂开多/平多单(基础间距 = max(atr_mult×ATR, 间距下限×价格)), 任一成交后成交价成为新平衡价,
--   按"成交后 1:1 恢复"的量全撤重挂两侧。
--   方向 flag(卖笔数 − 买笔数, 建仓与强平不计)驱动趋势侧间距指数放大: |flag|≥2 时 flag<0 放大买间距、
--   flag>0 放大卖间距, 间距 × flag_spacing_mult^(|flag|−1), 另一侧不变 —— 单边趋势成交变稀、
--   回调收割不变; 回调成交即 flag 回退、放大自收缩。买卖量随不对称挂单价自动跟随, 成交后仍精确回 1:1。
--
-- ═══ 合约要点 ═══
--   杠杆 1~5(默认 2, 清单 default_leverage 为唯一权威), 总资金 = 投入×杠杆; 手工改杠杆必须同步
--   [backtest].leverage, 否则 v_total 虚增被引擎拒单。
--   清算默认全仓(cross, 清单 [backtest].margin_mode): 投入资金整体作为清算缓冲, 爆仓距离按有效杠杆
--   (名义/投入)走 —— 半仓建仓时 5x 声明只决定可开名义上限。逐仓(--param margin_mode=isolated)
--   只认名义/杠杆划转的钱包保证金, 闲置现金不参与清算, 补仓资金救不了场。
--   v_pos 恒等于引擎实际多头仓位(逐 fill 同步); v_cash 隔离真实保证金随价格/浮盈亏的波动, 挂单量
--   以虚拟账本计算。卖平多的真实回笼(释放保证金+已实现盈亏)与虚拟记账(+q·p)合法分叉, 期末只核对
--   v_pos 与账本重建恒等式, 不核对 v_cash vs 引擎现金。
--   挂单带 position_side="long"(买=开多 / 卖=平多, 绝不反向开空); 平多量 cap 到实际持仓, 不自砍
--   (残仓致钱包 sweep 永不触发、保证金滞留停摆)。
--   爆仓监控: 每决策 bar 读 pos_liq, 距现价 < liq_warn_ratio 打 WARN(只告警不动作); cross 下单侧
--   爆仓价无定义(pos_liq 返回 nil → 策略侧告警自动静默, 引擎按组合权益实际清算)。运行期记录
--   距爆仓最近时刻(stat_liq_dist_*); 引擎强平 fill(LIQ- 前缀)触发即停机不再交易, 损失单列 stat_liq_pnl。
--   虚拟买单量超真实可用保证金时 WARN + 计数, 照常挂出(引擎侧保持 pending 不阻塞)。
--   CLOSE- 期末强平 fill 按普通卖出推进账本, 盈亏单列 stat_close_pnl; 不计 flag。
--   ⚠ 再平衡固有语义: 下跌后反弹的首笔平多单相对高位开多是保本/微亏, 属恒定权重特性而非配对违约。
--
-- ═══ 参数(清单 [[params]] 同步声明 default) ═══
--   pair              必填, 合约对(如 SOLUSDT)
--   start_price       必填, 开始价格(低于它才激活); 缺失/≤0 启动停机
--   invest_cash       投入资金(保证金), 默认 0(= 激活时 quote 余额全额)
--   interval          主时钟, 默认 1h(回测建议 1m 提升撮合粒度)
--   atr_interval      算 ATR 的 K 线周期, 默认 1h
--   atr_period        ATR 周期, 默认 14
--   atr_mult          基础间距 = atr_mult × ATR, 默认 1.5
--   min_spacing_pct   最小间距(占价格比例), 默认 0.002; 负数禁用下限
--   min_notional      单笔最小名义, 默认 5
--   fee_side          单边费率, 默认 0.0005
--   liq_warn_ratio    距爆仓价告警阈值, 默认 0.1
--   flag_spacing_mult 间距放大系数, 默认 1.2; 1.0 = 禁用放大(<1 按 1 处理)
--   (leverage 为引擎保留键, 走清单 default_leverage 回填, Lua 仅防御性校验 1~5)
--
-- ═══ 守卫 ═══
--   ATR 未就绪 → 不挂单(计数); 生效间距 < 4×fee_side×价格 → [FATAL] 停机(启动校验一次);
--   连续 1 天无任何挂单 → WARN。
--
-- ═══ 用法 ═══
--   ricow backtest --strategy shannon_grid_futures --pair SOLUSDT --interval 1m --days 180 \
--     --cash 10000 --param start_price=<窗口起点价> --param invest_cash=10000 --close-at-end
--   ricow run shannon_grid_futures --demo
--
quote_asset = ""  -- 计价资产, on_init 里 detect_quote(pair) 动态检测
built = false
balance_price = nil       -- 平衡价格(建仓成交价 / 最近一次网格成交价)
pending_entry = false     -- 建仓市价单在途
need_rehang = false       -- 成交/续接后置 true: 下一 tick 撤旧单 + 重挂两侧
halted = false            -- 停机标记(成本门槛/参数/爆仓)
fatal = 0                 -- 数值化停机标记(单测经 global_f64 读取)
cost_checked = false      -- 成本门槛只做一次启动校验
last_bar_ts = nil
invested0 = 0             -- 投入资金(激活时刻定格; 收益分解基准)
entry_price = 0           -- 初始建仓成交价(持仓损益基准)
entry_size = 0            -- 初始建仓数量
-- 虚拟账本: v_pos = 引擎实际多头仓位; v_cash 隔离保证金波动
v_cash = nil
v_pos = nil
v_total = 0               -- 总资金 = 投入 × 杠杆(激活时刻定格)
avg_entry = 0             -- 均值开仓成本(CLOSE- 强平盈亏口径)
-- 爆仓价监控
liq_warned = false
liq_price_final = 0       -- 期末最后一次读到的爆仓价
liq_dist_min = nil        -- 运行期距爆仓最小距离 (price−liq)/price; nil = 尚无有效爆仓价
liq_dist_min_ts = nil     -- 上述最小距离发生的 bar 时间戳 ms
liq_dist_min_price = 0    -- 上述最小距离发生时的现价
liq_count = 0             -- 引擎爆仓(LIQ- fill)次数
liq_pnl = 0               -- 爆仓损益(均值成本口径)
-- 停摆可观测性
stall_bars = 0            -- 处于"无任何挂单"状态的主时钟 bar 数(累计, 每 bar 去重 +1)
stall_bar_ts = nil        -- 上次 stall_bars +1 的 bar 时间戳 ms(同 bar 去重)
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
underfunded_buys = 0      -- 虚拟买单量超真实可用保证金的次数
underfunded_notional = 0  -- 上述买单的累计名义
close_pnl = 0             -- CLOSE- 强平锁定损益(均值成本口径)
m_fee = 0                 -- 虚拟账本累计手续费(Σ fill.fee)
buy_notional = 0          -- 虚拟账本累计买入名义(账本重建核对用)
sell_notional = 0         -- 虚拟账本累计卖出名义
last_price = 0
-- 方向 flag: 卖笔数 − 买笔数(建仓与强平不计), 驱动趋势侧间距指数放大
flag = 0
flag_max = 0              -- flag 历史最大值(最多卖出几个网格)
flag_min = 0              -- flag 历史最小值(负数, 最多买入几个网格)

local function num(ctx, key, dflt)
    local v = ctx:config_f64(key)
    if v == nil or v == 0 then
        return dflt
    end
    return v
end

-- 停摆 bar 计数(去重): 同一主时钟 bar 内多条路径只计一次。
local function bump_stall(ts)
    if ts ~= nil and ts ~= stall_bar_ts then
        stall_bar_ts = ts
        stall_bars = stall_bars + 1
    end
end

-- 开多量保证金兜底: 名义/杠杆 ≤ 可用余额(预扣费+滑点余量)。
local function cap_open(ctx, size, p, leverage, fee_bps)
    local cash = ctx:balance(quote_asset) or 0
    local max_sz = cash * leverage / (p * (1 + (fee_bps + 20) / 10000))
    if size > max_sz then
        return max_sz
    end
    return size
end

-- 平多量持仓兜底: cap 到引擎实际多头持仓即可, 不自砍(残仓致钱包 sweep 永不触发、保证金滞留停摆)。
local function cap_close(ctx, pair, size)
    local pos = ctx:pos_size(pair, "long") or 0
    if size > pos then
        return pos
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

-- 虚拟账本按真实成交推进: v_pos 与引擎逐分同步, v_cash 随每笔成交同步变化。
local function model_apply(ctx, fill, px, size)
    if v_cash == nil then
        return -- 建仓成交在 on_fill 里初始化账本, 不会走到这里
    end
    if fill.side == "buy" then
        avg_entry = (avg_entry * v_pos + size * px) / math.max(v_pos + size, 1e-12)
        v_cash = v_cash - size * px - (fill.fee or 0)
        v_pos = v_pos + size
        buy_notional = buy_notional + size * px
    else
        v_cash = v_cash + size * px - (fill.fee or 0)
        v_pos = v_pos - size
        sell_notional = sell_notional + size * px
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
    if v_cash ~= nil then
        ctx:state_set("v_cash", string.format("%.10f", v_cash))
    end
    ctx:state_set("avg_entry", string.format("%.10f", avg_entry))
    -- 虚拟账本重建核对恒等式的累计项(续接后 on_stop 核对仍成立):
    --   v_cash = v_total − m_fee − buy_notional + sell_notional
    ctx:state_set("m_fee", string.format("%.10f", m_fee))
    ctx:state_set("buy_notional", string.format("%.10f", buy_notional))
    ctx:state_set("sell_notional", string.format("%.10f", sell_notional))
    ctx:state_set("flag", string.format("%d", flag))
    ctx:state_set("halted", halted and "1" or "0")
    ctx:state_set("pending_entry", pending_entry and "1" or "0")
end

function on_init(ctx)
    local pair = ctx:config_str("pair")
    quote_asset = exec.detect_quote(pair)
    -- 数据需求声明: 主时钟 + ATR 序列(周期与根数由策略显式给出)。
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
    fatal = 0
    cost_checked = false
    v_cash = nil
    v_pos = nil
    v_total = 0
    avg_entry = 0
    liq_warned = false
    liq_price_final = 0
    liq_dist_min = nil
    liq_dist_min_ts = nil
    liq_dist_min_price = 0
    liq_count = 0
    liq_pnl = 0
    flag = 0
    flag_max = 0
    flag_min = 0

    -- 杠杆校验: 1~5 倍, 越界 FATAL 停机。杠杆唯一权威 = 清单 default_leverage;
    -- num() 将 0 折为默认, 下界用 <1 捕获。
    local leverage = num(ctx, "leverage", 2)
    if leverage < 1 or leverage > 5 then
        halted = true
        fatal = 1
        ctx:log(string.format(
            "[shannon_grid_futures] [FATAL] 杠杆越界 -> 停机: leverage=%.2f 必须在 1~5 之间 " ..
            "(默认 2; 与清单 default_leverage / [backtest].leverage 保持一致)", leverage))
        return
    end

    -- 断点续接: 引擎已把上次会话的状态注入(表 strategy_state)
    local bp = ctx:state_get("balance_price")
    if bp ~= nil and tonumber(bp) and tonumber(bp) > 0 then
        balance_price = tonumber(bp)
        built = true
        invested0 = tonumber(ctx:state_get("invested0")) or 0
        entry_price = tonumber(ctx:state_get("entry_price")) or 0
        entry_size = tonumber(ctx:state_get("entry_size")) or 0
        v_total = invested0 * leverage
        -- 虚拟账本: v_cash 从状态恢复, v_pos 与引擎实际持仓重新同步
        v_cash = tonumber(ctx:state_get("v_cash"))
        avg_entry = tonumber(ctx:state_get("avg_entry")) or 0
        m_fee = tonumber(ctx:state_get("m_fee")) or 0
        buy_notional = tonumber(ctx:state_get("buy_notional")) or 0
        sell_notional = tonumber(ctx:state_get("sell_notional")) or 0
        flag = tonumber(ctx:state_get("flag")) or 0
        v_pos = ctx:pos_size(pair, "long") or 0
        -- 账本完整性防御: 已建仓但虚拟现金缺失(状态损坏) → 记账链断裂,
        -- do_rehang 卖量公式会以 C=0 退化出 ≈Q/2 的半仓平多单 → 必须 FATAL 停机。
        if v_cash == nil or v_cash <= 0 then
            halted = true
            fatal = 1
            save_state(ctx)
            ctx:log("[shannon_grid_futures] [FATAL] 续接失败: 已建仓但状态中无有效 v_cash " ..
                "(虚拟账本不完整) -> 停机, 请人工核对持仓与保证金。")
            return
        end
        -- 重启后必须重挂(停机撤单兜底已清掉本策略挂单)
        need_rehang = true
        pending_entry = (ctx:state_get("pending_entry") == "1")
        halted = (ctx:state_get("halted") == "1")
        if halted then
            ctx:log("[shannon_grid_futures] 续接: 上次已停机(halted=true) -> 本次不再交易")
        end
        ctx:log(string.format(
            "[shannon_grid_futures] 续接上次状态: 平衡价 %.4f, 投入 %.2f×%.1f, 建仓 %.6f@%.4f, v_cash %.2f, flag=%d",
            balance_price, invested0, leverage, entry_size, entry_price, v_cash or 0, flag))
    end

    ctx:log(string.format(
        "[shannon_grid_futures] init pair=%s quote=%s 投入=%s 杠杆=%.1f(总资金=投入×杠杆) 建仓=总资金一半 " ..
        "主时钟=%s ATR=%s×%d mult=%.2f min_notional=%.2f fee_side=%.4f liq_warn=%.2f " ..
        "间距放大系数=%.2f(flag 驱动, 1.0=禁用) 保证金模式=%s",
        pair, quote_asset,
        (ctx:config_f64("invest_cash") or 0) > 0 and string.format("%.2f", ctx:config_f64("invest_cash"))
            or "激活时余额全额",
        leverage,
        ctx:config_str("interval") ~= nil and ctx:config_str("interval") ~= ""
            and ctx:config_str("interval") or "1h",
        atr_intv, num(ctx, "atr_period", 14), num(ctx, "atr_mult", 1.5),
        num(ctx, "min_notional", 5), num(ctx, "fee_side", 0.0005),
        num(ctx, "liq_warn_ratio", 0.1),
        num(ctx, "flag_spacing_mult", 1.2),
        ctx:config_str("margin_mode") or "isolated"))
    ctx:log(string.format(
        "[shannon_grid_futures] 激活: 价格低于 start_price=%.4f 才激活, 用总资金一半市价开多, " ..
        "成交价 = 第一平衡价格; 平衡价 ± ATR 挂开多/平多单, 成交后 1:1 恢复并重挂; " ..
        "⚠ 合约有杠杆, 请关注爆仓价告警",
        num(ctx, "start_price", 0)))
end

-- 全撤重挂(共享决策出口): on_fill 成交后与 on_tick 重挂共用。
-- C/Q = 虚拟账本 v_cash/v_pos; 挂单带 position_side="long"; 买量不砍但做真实可用保证金预估
-- (underfunded 可观测); 卖量 cap_close 不自砍; 两侧间距按方向 flag 指数放大。
-- 返回订单表(含 cancel_pending)或 {}(无单可挂)。
function do_rehang(ctx)
    local pair = ctx:config_str("pair")
    local price = ctx:price(pair)
    if not price or price <= 0 then
        return {}
    end
    local atr_mult = num(ctx, "atr_mult", 1.5)
    local min_notional = num(ctx, "min_notional", 5)
    local fee_side = num(ctx, "fee_side", 0.0005)
    local fee_bps = fee_side * 10000
    local f = fee_side
    local leverage = num(ctx, "leverage", 2)

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
        bump_stall((ctx:now() or {}).ts)
        if stall_since == nil then
            stall_since = (ctx:now() or {}).ts
        end
        return {}
    end
    -- 生效间距 = max(atr_mult×ATR, 最小间距下限×价格)
    local spacing = atr_mult * atr
    local min_spacing = num(ctx, "min_spacing_pct", 0.002) * price
    if min_spacing < 0 then
        min_spacing = 0 -- 负数 = 禁用下限
    end
    if spacing < min_spacing then
        spacing = min_spacing
    end
    -- 方向 flag 间距放大: 生效间距之上按 mult^(|flag|−1) 放大趋势侧
    local mult = num(ctx, "flag_spacing_mult", 1.2)
    local buy_spacing, sell_spacing = side_spacings(spacing, flag, mult)

    -- C/Q = 虚拟账本
    local C = v_cash or 0
    local Q = v_pos or 0
    local buy_px = balance_price - buy_spacing
    local sell_px = balance_price + sell_spacing
    -- 漂移纠偏: 引擎对跳空 bar 按开盘价成交使账本逐次漂移; 买侧漂移过度时 C ≤ Q×买价 → 买量出负
    -- → 不挂买单 → 仅剩上方卖单, 下跌行情卖单永不成交 → 平衡价冻结 → 死锁。
    -- 触发即把平衡价重锚到 C/Q(1:1 恒等式定义价), 两侧恢复挂单。无漂移时不触发,
    -- "成交后平衡价 := 成交价"语义不变。
    -- ⚠ 仅纠偏买侧: 卖侧 C ≥ Q×卖价(卖量出负)是合法的 underfunded 观测场景, 且上涨时下方
    -- 买单必成交解冻, 不得纠偏。
    -- ⚠ 重锚后重算挂单价必须用放大后的买/卖间距, 与实际挂单一致。
    if Q > 0 and C > 0 and C <= Q * buy_px then
        balance_price = C / Q
        buy_px = balance_price - buy_spacing
        sell_px = balance_price + sell_spacing
    end
    local orders = { { pair = pair, action = "cancel_pending" } }

    if buy_px > 0 then
        local q = (C - Q * buy_px) / (buy_px * (2 + f))
        if q > 0 then
            -- 买单量不砍: 估算占用保证金超真实可用 → 记录可观测, 照常挂出
            -- (引擎侧资金不足限价单保持 pending, 不阻塞)。
            local avail = ctx:balance(quote_asset) or 0
            local need_cash = q * buy_px * (1 + (fee_bps + 20) / 10000) / leverage
            if need_cash > avail then
                underfunded_buys = underfunded_buys + 1
                underfunded_notional = underfunded_notional + q * buy_px
                ctx:log(string.format(
                    "[shannon_grid_futures] [WARN] 虚拟买单超真实可用: 需保证金 %.2f > 可用 %.2f " ..
                    "(名义 %.2f, 杠杆 %.1f) -> 照常挂出, 待引擎资金就绪成交",
                    need_cash, avail, q * buy_px, leverage))
            end
            if q * buy_px >= min_notional then
                orders[#orders + 1] = {
                    pair = pair, side = "buy", size = q, order_type = "limit", price = buy_px,
                    position_side = "long",
                }
            end
        end
    else
        skip_zero_buy_px = skip_zero_buy_px + 1
    end

    if Q > 0 then
        local q = (Q * sell_px - C) / (sell_px * (2 - f))
        if q > 0 then
            q = cap_close(ctx, pair, q)
            if q > 0 and q * sell_px >= min_notional then
                orders[#orders + 1] = {
                    pair = pair, side = "sell", size = q, order_type = "limit", price = sell_px,
                    position_side = "long",
                }
            end
        end
    end

    if #orders <= 1 then
        -- 两侧都没挂出(量太小/名义不足/单侧资金耗尽): 保持 need_rehang 下一 tick 再试(防永久停摆)
        skip_notional = skip_notional + 1
        bump_stall((ctx:now() or {}).ts)
        if stall_since == nil then
            stall_since = (ctx:now() or {}).ts
        end
        if stall_since ~= nil and (ctx:now() or {}).ts ~= nil
            and (ctx:now()).ts - stall_since >= 86400000
            and (stall_warned_at == nil or (ctx:now()).ts - stall_warned_at >= 86400000) then
            stall_warned_at = (ctx:now()).ts
            ctx:log(string.format(
                "[shannon_grid_futures] [WARN] 停摆 ≥1 天: 平衡价 %.4f 间距 %.4f 下两侧均无法挂单 " ..
                "(虚拟现金 %.2f 虚拟持仓 %.6f, 小名义跳过 %d 次)",
                balance_price, spacing, C, Q, skip_notional))
        end
        return {}
    end

    need_rehang = false
    rehang_count = rehang_count + 1
    save_state(ctx)
    ctx:log(string.format(
        "[shannon_grid_futures] 网格重挂 #%d: 平衡价 %.4f 基础间距 %.4f (ATR %.4f×%.2f) flag=%d " ..
        "买间距 %.4f / 卖间距 %.4f -> 买 %s / 卖 %s | 虚拟现金 %.2f 虚拟持仓 %.6f",
        rehang_count, balance_price, spacing, atr, atr_mult, flag, buy_spacing, sell_spacing,
        buy_px > 0 and string.format("%.4f", buy_px) or "跳过(买价≤0)",
        Q > 0 and string.format("%.4f", sell_px) or "无(无持仓)",
        C, Q))
    return orders
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

    local atr_mult = num(ctx, "atr_mult", 1.5)
    local min_notional = num(ctx, "min_notional", 5)
    local fee_side = num(ctx, "fee_side", 0.0005)
    local fee_bps = fee_side * 10000
    local f = fee_side
    local leverage = num(ctx, "leverage", 2)

    -- 爆仓价监控(每决策 bar 更新): 多头距爆仓价 < liq_warn_ratio 打 WARN,
    -- 只告警不动作(去重防刷屏); 距离回升 1.5× 重置。
    local pos_now = ctx:pos_size(pair, "long") or 0
    if pos_now > 0 then
        local liq = ctx:pos_liq(pair, "long")
        if liq and liq > 0 then
            liq_price_final = liq
            -- 最近爆仓距离追踪: 只收紧不回退
            local dist_all = (price - liq) / price
            if liq_dist_min == nil or dist_all < liq_dist_min then
                liq_dist_min = dist_all
                liq_dist_min_ts = t.ts
                liq_dist_min_price = price
            end
            local liq_ratio = num(ctx, "liq_warn_ratio", 0.1)
            local dist = dist_all
            if dist < liq_ratio then
                if not liq_warned then
                    liq_warned = true
                    ctx:log(string.format(
                        "[shannon_grid_futures] [WARN] ⚠ 多头逼近爆仓价: 现价 %.4f 爆仓价 %.4f 距离 %.2f%% (< %.2f%%) " ..
                        "—— 合约杠杆风险, 请注意减仓或追加保证金",
                        price, liq, dist * 100, liq_ratio * 100))
                end
            elseif dist >= liq_ratio * 1.5 then
                liq_warned = false -- 距离回升: 重置, 下次再逼近再告警
            end
        end
    else
        liq_warned = false
    end

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
        bump_stall(t.ts)
        if stall_since == nil then
            stall_since = t.ts
        end
        if t.ts - stall_since >= 86400000 and (stall_warned_at == nil or t.ts - stall_warned_at >= 86400000) then
            stall_warned_at = t.ts
            ctx:log(string.format(
                "[shannon_grid_futures] [WARN] 停摆 ≥1 天: ATR 未就绪已连续跳过 %d 根 bar, 不挂单",
                skip_no_atr))
        end
        return {}
    end
    -- 生效间距 = max(atr_mult×ATR, 最小间距下限×价格); 下限防 ATR 过窄被手续费磨损
    local spacing = atr_mult * atr
    local min_spacing = num(ctx, "min_spacing_pct", 0.002) * price
    if min_spacing < 0 then
        min_spacing = 0 -- 负数 = 禁用下限
    end
    if spacing < min_spacing then
        spacing = min_spacing
    end
    -- 死锁检测须用与实际挂单一致的放大后买间距
    local buy_spacing = side_spacings(spacing, flag, num(ctx, "flag_spacing_mult", 1.2))

    -- 成本门槛(启动硬校验一次): 生效间距必须 ≥ 4×单边费率(严格小于才停机)
    if not cost_checked then
        cost_checked = true
        local need = 4 * fee_side * price
        if spacing < need then
            halted = true
            fatal = 1
            save_state(ctx)
            ctx:log(string.format(
                "[shannon_grid_futures] [FATAL] 成本门槛不满足 -> 停机: 生效间距=%.6f (=%.4f%% 价格) " ..
                "必须 ≥ 4×fee_side×价格=%.6f (=%.4f%%); atr_mult=%.2f ATR=%.6f 价格=%.4f",
                spacing, spacing / price * 100, need, need / price * 100, atr_mult, atr, price))
            return {}
        end
        ctx:log(string.format(
            "[shannon_grid_futures] 成本门槛通过(首次校验): 生效间距=%.6f (%.4f%% 价格) ≥ %.4f%%",
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
            fatal = 1
            save_state(ctx)
            ctx:log("[shannon_grid_futures] [FATAL] 缺少必填参数 start_price(开始价格) -> 停机: " ..
                "策略只在价格低于 start_price 时激活。")
            return {}
        end
        if price >= start_px then
            return {}
        end
        -- 激活: 总资金(投入×杠杆)一半市价开多(invest_cash=0 → 激活时 quote 余额全额视为投入)
        local inv = ctx:config_f64("invest_cash") or 0
        if inv <= 0 then
            inv = ctx:balance(quote_asset) or 0
        end
        v_total = inv * leverage
        local notional = v_total / 2
        local size = cap_open(ctx, notional / price, price, leverage, fee_bps)
        if size > 0 and size * price >= min_notional then
            pending_entry = true
            invested0 = inv
            ctx:log(string.format(
                "[shannon_grid_futures] 激活(价 %.4f < 开始价 %.4f) -> 市价开多 %.6f (≈%.2f %s, " ..
                "总资金 %.2f = 投入 %.2f × 杠杆 %.1f 的一半), 成交价将成为第一平衡价格",
                price, start_px, size, size * price, quote_asset, v_total, inv, leverage))
            return {
                { pair = pair, side = "buy", size = size, order_type = "market", position_side = "long" },
            }
        end
        ctx:log(string.format(
            "[shannon_grid_futures] 激活但建仓名义 %.2f < min_notional %.2f -> 按无建仓处理(平衡价 := 现价)",
            size * price, min_notional))
        built = true
        balance_price = price
        v_total = inv * leverage
        v_cash = v_total
        v_pos = 0
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

    -- 死锁检测: 买侧漂移死锁时 C ≤ Q×买价 → 买单挂不出, 仅剩上方卖单, 下跌行情永不成交
    -- → need_rehang 永假 → do_rehang 的纠偏分支永远执行不到。
    -- 按与 do_rehang 相同的触发条件强制置 need_rehang, 使纠偏生效解冻网格
    -- (买价用放大后的买间距, 与 do_rehang 实际挂单价一致)。
    if not need_rehang and balance_price ~= nil and (v_pos or 0) > 0 and (v_cash or 0) > 0
        and (v_cash or 0) <= (v_pos or 0) * (balance_price - buy_spacing) then
        need_rehang = true
    end

    if not need_rehang then
        return {} -- 两侧单未成交 → 平衡价不变, 不重挂
    end

    return do_rehang(ctx)
end

function on_fill(ctx, fill)
    -- 多头侧成交(防御: 非 long 一律忽略 —— 本策略无对冲腿, 不应出现 short 侧成交)
    if fill.position_side and fill.position_side ~= "long" then
        return {}
    end
    local px = fill.fill_price or balance_price or 0
    local size = fill.fill_size or 0
    local pair = ctx:config_str("pair")

    -- 爆仓判定: 引擎强平 fill 的 client_order_id 以 "LIQ-" 开头(hedge: LIQ-{pair}-long)。
    -- ⚠ 引擎强平 fill 的 side 语义是"追加该方向"口径(平多报 buy), 不能按 side 走 model_apply
    -- —— 直接以引擎仓位为真值同步。
    if (fill.client_order_id or ""):sub(1, 4) == "LIQ-" then
        liq_count = liq_count + 1
        liq_pnl = liq_pnl + size * (px - avg_entry)  -- 均值成本口径的爆仓损失(负值)
        if fill.side == "buy" then
            buy_count = buy_count + 1
        else
            sell_count = sell_count + 1
        end
        fill_count = fill_count + 1
        -- 账本以引擎为真值: 强平后引擎整侧仓位清零, v_pos 同步; v_cash 定格(爆仓后不再交易)
        v_pos = ctx:pos_size(pair, "long") or 0
        -- 爆仓即停机: 持久化 halted, 重启不复活交易
        halted = true
        fatal = 1
        save_state(ctx)
        ctx:log(string.format(
            "[shannon_grid_futures] [FATAL] 💥 爆仓 -> 停机: 强平成交 %.6f @ %.4f (费 %.4f) | " ..
            "虚拟现金 %.2f 虚拟持仓 %.6f | 开仓口径估算爆仓价 %.4f | " ..
            "运行期最近爆仓距离 %s(发生时价格 %.4f) | 已全撤在途挂单, 不再交易",
            size, px, fill.fee or 0, v_cash or 0, v_pos or 0, liq_price_final,
            liq_dist_min and string.format("%.2f%%", liq_dist_min * 100) or "N/A",
            liq_dist_min_price))
        return { { pair = pair, action = "cancel_pending" } }
    end

    if pending_entry then
        -- 建仓成交: 成交价 = 第一平衡价格; 虚拟账本在此初始化:
        -- v_pos = 实际开仓量; v_cash = v_total − 建仓名义 − 费(总资金另一半留在虚拟现金侧)。
        pending_entry = false
        built = true
        balance_price = px
        entry_price = px
        entry_size = size
        avg_entry = px
        v_pos = ctx:pos_size(pair, "long") or size
        v_cash = v_total - size * px - (fill.fee or 0)
        buy_notional = size * px
        m_fee = fill.fee or 0
        -- 建仓也是真实成交: 计入成交统计(不计 flag)
        if fill.side == "buy" then
            buy_count = buy_count + 1
        else
            sell_count = sell_count + 1
        end
        fill_count = fill_count + 1
        need_rehang = true
        save_state(ctx)
        ctx:log(string.format(
            "[shannon_grid_futures] 建仓成交 %.6f @ %.4f (费 %.4f) -> 平衡价 := %.4f | " ..
            "虚拟现金 %.2f 虚拟持仓 %.6f 仓位市值 %.2f (总资金 %.2f = 投入 %.2f × 杠杆)",
            size, px, fill.fee or 0, balance_price, v_cash or 0, v_pos or 0, (v_pos or 0) * px,
            v_total, invested0))
        -- 建仓成交立即全撤重挂
        if halted then
            return {}
        end
        return do_rehang(ctx)
    end

    -- 网格成交: 平衡价 := 成交价; 虚拟账本按真实成交推进; CLOSE- 强平单盈亏单列(均值成本口径)
    local is_close = (fill.client_order_id or ""):sub(1, 6) == "CLOSE-"
    if fill.side == "sell" and is_close then
        close_pnl = close_pnl + size * (px - avg_entry)
    end
    balance_price = px
    model_apply(ctx, fill, px, size)
    if fill.side == "buy" then
        buy_count = buy_count + 1
    else
        sell_count = sell_count + 1
    end
    -- 方向 flag 计数: 买 −1 / 卖 +1; CLOSE- 期末强平不计(非网格主动成交)。
    -- 建仓/LIQ- 分支在上方各自独立处理, 均不计数。
    if not is_close then
        if fill.side == "buy" then
            flag = flag - 1
        else
            flag = flag + 1
        end
        if flag > flag_max then
            flag_max = flag
        end
        if flag < flag_min then
            flag_min = flag
        end
    end
    fill_count = fill_count + 1
    need_rehang = true
    save_state(ctx)
    ctx:log(string.format(
        "[FILL] #%d %s size=%.6f px=%.4f notional=%.2f 费=%.4f | flag=%d 虚拟现金 %.2f 虚拟持仓 %.6f " ..
        "平衡价(新)=%.4f 1:1 检查: 虚拟现金/仓位市值 = %.4f",
        fill_count, fill.side, size, px, size * px, fill.fee or 0, flag,
        v_cash or 0, v_pos or 0, balance_price,
        (v_pos and v_pos > 0 and v_pos * px > 0) and (v_cash or 0) / (v_pos * px) or 0))
    -- 网格成交立即全撤重挂(平衡价 := 本笔成交价)。
    -- halted 后仍记账(保持 v_pos 与引擎同步)但不再产出订单。
    if halted then
        return {}
    end
    return do_rehang(ctx)
end

-- 拒单回传: rejected → 重挂防停摆(cancelled 是策略主动撤单的正常回传, 不处理)。
function on_order_update(ctx, upd)
    local status = upd and upd.status
    if status == "rejected" then
        if pending_entry then
            pending_entry = false -- 建仓被拒: 恢复, 下个 tick 重新走激活
        end
        need_rehang = true
        ctx:log(string.format(
            "[shannon_grid_futures] 挂单被拒: pair=%s filled=%.6f remaining=%.6f → 重挂",
            upd.pair, upd.filled_size or 0, upd.remaining_size or 0))
    end
end

function on_stop(ctx)
    local pair = ctx:config_str("pair")
    local pos = ctx:pos_size(pair, "long") or 0
    local eq = ctx:equity() or 0

    -- 对账: v_pos vs 引擎持仓应逐分一致(差 > 0.01 → WARN);
    -- v_cash 与引擎现金本就合法分叉(卖平多真实回笼 = 释放保证金 + 已实现盈亏, 虚拟记 +q·p;
    -- 资金费引擎计收、虚拟账本不计), 只留档不告警。
    local d_pos = (v_pos or pos) - pos
    if math.abs(d_pos) > 0.01 then
        ctx:log(string.format(
            "[shannon_grid_futures] [WARN] 持仓分叉: 虚拟持仓 %.8f vs 引擎 %.8f (差 %.8f)",
            v_pos or 0, pos, d_pos))
    end

    -- 虚拟账本重建核对: v_cash 应恒等于 总资金 − Σ费 − Σ买名义 + Σ卖名义
    -- (逐 fill 递推的封闭恒等式; 再平衡浮盈合法进入 v_cash)。差 > 0.01 → WARN。
    local v_recon = v_total - m_fee - buy_notional + sell_notional
    local v_recon_diff = (v_cash or 0) - v_recon
    if v_total > 0 and math.abs(v_recon_diff) > 0.01 then
        ctx:log(string.format(
            "[shannon_grid_futures] [WARN] 虚拟账本重建核对失败: v_cash %.4f vs 重建 %.4f (差 %.4f)",
            v_cash or 0, v_recon, v_recon_diff))
    end

    -- 收益分解: 合计 = 期末权益 − 投入 = 持仓收益(期末持仓×(末价−建仓价)) + 交易收益(再平衡净贡献)
    local out = string.format(
        "[shannon_grid_futures] 停机(**不清仓**): 成交 %d(买 %d 卖 %d) / 跳过(ATR未就绪 %d, 小名义 %d, 买价≤0 %d) / " ..
        "重挂 %d / 资金不足买 %d 次(累计名义 %.2f) / flag=%d(峰值 +%d/%d) / %s / 平衡价 %s / " ..
        "期末虚拟现金 %.2f 持仓 %.6f(市值 %.2f) " ..
        "权益 %.2f / 虚拟费 %.4f / 强平锁定 %.2f / 爆仓 %d 次(损失 %.2f, 最近距离 %s) / 持仓差 %.8f",
        fill_count, buy_count, sell_count, skip_no_atr, skip_notional, skip_zero_buy_px,
        rehang_count, underfunded_buys, underfunded_notional,
        flag, flag_max, flag_min,
        halted and "已停机(成本门槛/参数/爆仓)" or "正常运行",
        balance_price and string.format("%.4f", balance_price) or "nil",
        v_cash or 0, pos, pos * (last_price or 0), eq, m_fee, close_pnl,
        liq_count, liq_pnl,
        liq_dist_min and string.format("%.2f%%", liq_dist_min * 100) or "N/A", d_pos)
    if invested0 > 0 and last_price > 0 then
        local total = eq - invested0
        local hold = 0
        if entry_price > 0 then
            hold = pos * (last_price - entry_price)
        end
        out = out .. string.format(
            " | 收益分解: 投入 %.2f × 杠杆 | 合计 %+.2f = 持仓 %+.2f + 交易(再平衡) %+.2f",
            invested0, total, hold, total - hold)
    end
    ctx:log(out)

    -- stat_* 导出(报告"策略统计"区块)
    ctx:state_set("stat_eff_leverage", string.format("%.1f", num(ctx, "leverage", 2)))
    ctx:state_set("stat_eff_atr_interval", ctx:config_str("atr_interval") or "1h")
    ctx:state_set("stat_eff_atr_period",
        string.format("%d", math.floor(num(ctx, "atr_period", 14))))
    ctx:state_set("stat_eff_atr_mult", string.format("%.4f", num(ctx, "atr_mult", 1.5)))
    ctx:state_set("stat_eff_min_spacing_pct",
        string.format("%.4f", num(ctx, "min_spacing_pct", 0.002)))
    ctx:state_set("stat_eff_min_notional", string.format("%.2f", num(ctx, "min_notional", 5)))
    ctx:state_set("stat_eff_fee_side", string.format("%.4f", num(ctx, "fee_side", 0.0005)))
    ctx:state_set("stat_invested0", string.format("%.2f", invested0))
    ctx:state_set("stat_v_total", string.format("%.2f", v_total))
    ctx:state_set("stat_entry_price", string.format("%.6f", entry_price))
    ctx:state_set("stat_entry_size", string.format("%.6f", entry_size))
    ctx:state_set("stat_fill_count", string.format("%d", fill_count))
    ctx:state_set("stat_buy_count", string.format("%d", buy_count))
    ctx:state_set("stat_sell_count", string.format("%d", sell_count))
    ctx:state_set("stat_rehang_count", string.format("%d", rehang_count))
    ctx:state_set("stat_skip_no_atr", string.format("%d", skip_no_atr))
    ctx:state_set("stat_skip_notional", string.format("%d", skip_notional))
    ctx:state_set("stat_underfunded_buys", string.format("%d", underfunded_buys))
    ctx:state_set("stat_underfunded_notional", string.format("%.2f", underfunded_notional))
    ctx:state_set("stat_stall_bars", string.format("%d", stall_bars))
    ctx:state_set("stat_cash_final", string.format("%.2f", ctx:balance(quote_asset) or 0))
    ctx:state_set("stat_v_cash_final", string.format("%.2f", v_cash or 0))
    ctx:state_set("stat_pos_final", string.format("%.8f", pos))
    ctx:state_set("stat_pos_value_final", string.format("%.2f", pos * (last_price or 0)))
    ctx:state_set("stat_balance_price",
        balance_price and string.format("%.6f", balance_price) or "")
    ctx:state_set("stat_fee_total", string.format("%.4f", m_fee))
    ctx:state_set("stat_close_pnl", string.format("%.4f", close_pnl))
    ctx:state_set("stat_ledger_diff_pos", string.format("%.8f", d_pos))
    ctx:state_set("stat_v_equity_final", string.format("%.2f", (v_cash or 0) + (v_pos or 0) * (last_price or 0)))
    ctx:state_set("stat_v_recon_diff", string.format("%.6f", v_recon_diff))
    ctx:state_set("stat_liq_price_final", string.format("%.6f", liq_price_final))
    -- 爆仓风险可观测: 运行期最近爆仓距离 + 爆仓事件
    ctx:state_set("stat_liq_dist_min",
        liq_dist_min and string.format("%.6f", liq_dist_min * 100) or "")
    ctx:state_set("stat_liq_dist_min_ts",
        liq_dist_min_ts and string.format("%d", liq_dist_min_ts) or "")
    ctx:state_set("stat_liq_dist_min_price", string.format("%.6f", liq_dist_min_price))
    ctx:state_set("stat_liq_count", string.format("%d", liq_count))
    ctx:state_set("stat_liq_pnl", string.format("%.4f", liq_pnl))
    ctx:state_set("stat_halted", halted and "1" or "0")
    -- 方向 flag 可观测
    ctx:state_set("stat_eff_flag_spacing_mult",
        string.format("%.4f", num(ctx, "flag_spacing_mult", 1.2)))
    ctx:state_set("stat_flag", string.format("%d", flag))
    ctx:state_set("stat_flag_max", string.format("%d", flag_max))
    ctx:state_set("stat_flag_min", string.format("%d", flag_min))
end
