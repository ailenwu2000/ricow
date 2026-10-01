-- 合约中性香农网格 (shannon_neutral_grid_futures)
--
-- ═══ 语义 ═══
--   USDT-M 合约单向持仓(one-way, 可多可空)香农网格: 启动不建仓, 虚拟账本(现金 v_cash + 仓位 v_pos)
--   自 start_price 起即 1:1 价值平衡: 总资金 = 投入×杠杆, 虚拟仓位 Q0 = 总资金/(2×start_price),
--   虚拟现金 = 总资金/2, 平衡价 = start_price。真实净仓 = v_pos − Q0, 围绕 0 双向摆动 ——
--   策略目标是实际仓位尽量保持 0(或随网格收割缓慢增长), 双向收割波动。
--   平衡价 − 买间距挂买单 / 平衡价 + 卖间距挂卖单(基础间距 = max(atr_mult×ATR, 间距下限×价格)),
--   挂单量按"成交后虚拟现金与虚拟仓位价值恰好恢复 1:1"(含手续费修正)计算; 任一成交后成交价成为
--   新平衡价, 全撤重挂两侧。卖单不带 position_side/reduce_only —— 空仓卖出即开空, 是设计意图。
--   方向 flag(卖笔数 − 买笔数, 强平/期末清仓不计)驱动趋势侧间距指数放大: |flag|≥2 时 flag<0 放大
--   买间距、flag>0 放大卖间距, 间距 × flag_spacing_mult^(|flag|−1), 另一侧不变 —— 单边趋势成交变稀、
--   回调收割间距不变、更容易回调; 回调成交即 flag 回退、放大自收缩。买卖量随不对称挂单价自动跟随,
--   成交后仍精确回 1:1。
--
-- ═══ 合约要点 ═══
--   杠杆 1~5(默认 3, 清单 default_leverage 为唯一权威), 总资金 = 投入×杠杆; 手工改杠杆必须同步
--   [backtest].leverage, 否则 v_total 虚增被引擎拒单。
--   清算默认全仓(cross, 清单 [backtest].margin_mode): 投入资金整体作为清算缓冲, 净敞口 = |多−空|
--   远小于总名义, 双向持仓盈亏互抵。逐仓(--param margin_mode=isolated)两腿独立清算。
--   v_pos 恒等于 Q0 + 引擎真实净仓(逐 fill 同步); v_cash 隔离真实权益随价格/浮盈亏的波动, 挂单量
--   以虚拟账本计算。真实回笼(释放保证金+已实现盈亏)与虚拟记账(+q·p)合法分叉, 期末核对
--   v_pos 净仓恒等式与虚拟账本重建恒等式, 不核对 v_cash vs 引擎现金。
--   漂移纠偏(双侧): 引擎对跳空 bar 按开盘价成交使账本逐次漂移, 买侧漂移过度(C ≤ Q×买价)→ 买量出负
--   → 仅剩上方卖单 → 下跌永不成交 → 平衡价冻结死锁; 卖侧镜像(C ≥ Q×卖价)→ 卖量出负 → 仅剩下方
--   买单 → 上涨永不成交 → 同样死锁。任一触发即把平衡价重锚到 C/Q(1:1 恒等式定义价), 两侧恢复挂单;
--   无漂移时不触发, "成交后平衡价 := 成交价"语义逐分不变。重锚与死锁检测必须用放大后的买/卖间距,
--   与实际挂单价逐字一致。
--   爆仓监控: 每决策 bar 读两侧 pos_liq, 距现价 < liq_warn_ratio 打 WARN(只告警不动作); cross 下
--   pos_liq 返回 nil → 告警自动静默, 引擎按组合权益实际清算。运行期记录距爆仓最近时刻
--   (stat_liq_dist_*); 引擎强平 fill(LIQ- 前缀)触发即停机不再交易, 损失按自维护逐向均价单列
--   stat_liq_pnl。
--   挂单带真实资金预估: 虚拟买单量超真实可用保证金 → WARN + 计数, 照常挂出(引擎侧保持 pending)。
--   CLOSE- 期末强平 fill 按普通成交推进账本, 盈亏按被平侧自维护均价单列 stat_close_pnl; 不计 flag。
--   ⚠ 再平衡固有语义: 穿越平衡价方向的成交相对原持仓可能是保本/微亏, 属恒定权重特性而非配对违约。
--
-- ═══ 参数(清单 [[params]] 同步声明 default) ═══
--   pair              必填, 合约对(如 SOLUSDT)
--   start_price       必填, 开始价格 = 初始平衡价与虚拟仓位基准(回测设窗口起点价, 勿前视)
--   invest_cash       投入资金(保证金), 默认 0(= 启动时 quote 余额全额)
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
--   ricow backtest --strategy shannon_neutral_grid_futures --pair SOLUSDT --interval 1m --days 180 \
--     --cash 10000 --param start_price=<窗口起点价> --param invest_cash=10000 --close-at-end
--   ricow run shannon_neutral_grid_futures --demo

quote_asset = ""  -- 计价资产, on_init 里 detect_quote(pair) 动态检测
built = false
balance_price = nil       -- 平衡价格(最近一次网格成交价 / 漂移纠偏重锚价)
need_rehang = false       -- 成交/续接/纠偏后置 true: 下一 tick 撤旧单 + 重挂两侧
halted = false            -- 停机标记(成本门槛/参数/爆仓)
fatal = 0                 -- 数值化停机标记(单测经 global_f64 读取)
cost_checked = false      -- 成本门槛只做一次启动校验
last_bar_ts = nil
invested0 = 0             -- 投入资金(启动时刻定格; 收益分解基准)
-- 虚拟账本: v_pos = Q0 + 引擎真实净仓; v_cash 隔离权益波动
v_cash = nil
v_pos = nil
v_total = 0               -- 总资金 = 投入 × 杠杆(启动时刻定格)
v_init = 0                -- 初始虚拟现金 = v_total/2(账本重建恒等式基准)
q0 = 0                    -- 初始虚拟仓位 = 总资金/(2×start_price)
-- 真实方向仓自维护(镜像引擎 one-way 先平后开语义; 用于 CLOSE-/LIQ- 盈亏口径)
r_long = 0
r_short = 0
avg_l = 0                 -- 多头加权开仓均价
avg_s = 0                 -- 空头加权开仓均价
-- 爆仓价监控
liq_warned = false
liq_price_final = 0
liq_dist_min = nil        -- 运行期距爆仓最小距离 (price−liq)/price; nil = 尚无有效爆仓价
liq_dist_min_ts = nil
liq_dist_min_price = 0
liq_count = 0             -- 引擎爆仓(LIQ- fill)次数
liq_pnl = 0               -- 爆仓损益(自维护均价口径)
-- 停摆可观测性
stall_bars = 0
stall_bar_ts = nil
stall_since = nil
stall_warned_at = nil
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
close_pnl = 0             -- CLOSE- 期末强平锁定损益(自维护均价口径)
m_fee = 0                 -- 虚拟账本累计手续费(Σ fill.fee)
buy_notional = 0          -- 虚拟账本累计买入名义(账本重建核对用)
sell_notional = 0         -- 虚拟账本累计卖出名义
last_price = 0
max_net_notional = 0      -- 运行期最大真实净名义 |多−空|×价
-- 方向 flag: 卖笔数 − 买笔数(强平/期末清仓不计), 驱动趋势侧间距指数放大
flag = 0
flag_max = 0
flag_min = 0

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

-- 真实方向仓推进(镜像引擎 one-way 先平后开): 买先平空余量开多, 卖先平多余量开空;
-- 加权均价只在开仓腿更新。
local function real_apply(side, size, px)
    if side == "buy" then
        local close_sz = math.min(size, r_short)
        if close_sz > 0 then
            r_short = r_short - close_sz
            if r_short <= 1e-12 then
                r_short, avg_s = 0, 0
            end
        end
        local open_sz = size - close_sz
        if open_sz > 0 then
            avg_l = (avg_l * r_long + open_sz * px) / (r_long + open_sz)
            r_long = r_long + open_sz
        end
    else
        local close_sz = math.min(size, r_long)
        if close_sz > 0 then
            r_long = r_long - close_sz
            if r_long <= 1e-12 then
                r_long, avg_l = 0, 0
            end
        end
        local open_sz = size - close_sz
        if open_sz > 0 then
            avg_s = (avg_s * r_short + open_sz * px) / (r_short + open_sz)
            r_short = r_short + open_sz
        end
    end
end

-- 虚拟账本按真实成交推进: v_pos 与引擎净仓逐分同步, v_cash 随每笔成交同步变化。
local function model_apply(fill, px, size)
    if fill.side == "buy" then
        v_cash = v_cash - size * px - (fill.fee or 0)
        v_pos = v_pos + size
        buy_notional = buy_notional + size * px
    else
        v_cash = v_cash + size * px - (fill.fee or 0)
        v_pos = v_pos - size
        sell_notional = sell_notional + size * px
    end
    m_fee = m_fee + (fill.fee or 0)
    real_apply(fill.side, size, px)
end

function save_state(ctx)
    if balance_price ~= nil then
        ctx:state_set("balance_price", string.format("%.10f", balance_price))
    end
    ctx:state_set("built", built and "1" or "0")
    ctx:state_set("invested0", string.format("%.2f", invested0))
    ctx:state_set("q0", string.format("%.10f", q0))
    ctx:state_set("v_init", string.format("%.10f", v_init))
    if v_cash ~= nil then
        ctx:state_set("v_cash", string.format("%.10f", v_cash))
    end
    -- 虚拟账本重建核对恒等式的累计项(续接后 on_stop 核对仍成立):
    --   v_cash = v_init − m_fee − buy_notional + sell_notional
    ctx:state_set("m_fee", string.format("%.10f", m_fee))
    ctx:state_set("buy_notional", string.format("%.10f", buy_notional))
    ctx:state_set("sell_notional", string.format("%.10f", sell_notional))
    ctx:state_set("flag", string.format("%d", flag))
    ctx:state_set("halted", halted and "1" or "0")
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
    need_rehang = false
    halted = false
    fatal = 0
    cost_checked = false
    v_cash = nil
    v_pos = nil
    v_total = 0
    v_init = 0
    q0 = 0
    r_long = 0
    r_short = 0
    avg_l = 0
    avg_s = 0
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
    local leverage = num(ctx, "leverage", 3)
    if leverage < 1 or leverage > 5 then
        halted = true
        fatal = 1
        ctx:log(string.format(
            "[shannon_neutral_grid_futures] [FATAL] 杠杆越界 -> 停机: leverage=%.2f 必须在 1~5 之间 " ..
            "(默认 3; 与清单 default_leverage / [backtest].leverage 保持一致)", leverage))
        return
    end

    local start_px = num(ctx, "start_price", 0)
    if start_px <= 0 then
        halted = true
        fatal = 1
        ctx:log("[shannon_neutral_grid_futures] [FATAL] 缺少必填参数 start_price(开始价格) -> 停机: " ..
            "start_price 是初始平衡价与虚拟仓位基准。")
        return
    end

    -- 断点续接: 引擎已把上次会话的状态注入(表 strategy_state)
    local bp = ctx:state_get("balance_price")
    if bp ~= nil and tonumber(bp) and tonumber(bp) > 0 and (ctx:state_get("built") == "1") then
        balance_price = tonumber(bp)
        built = true
        invested0 = tonumber(ctx:state_get("invested0")) or 0
        q0 = tonumber(ctx:state_get("q0")) or 0
        v_total = invested0 * leverage
        v_init = tonumber(ctx:state_get("v_init")) or (v_total / 2)
        v_cash = tonumber(ctx:state_get("v_cash"))
        m_fee = tonumber(ctx:state_get("m_fee")) or 0
        buy_notional = tonumber(ctx:state_get("buy_notional")) or 0
        sell_notional = tonumber(ctx:state_get("sell_notional")) or 0
        flag = tonumber(ctx:state_get("flag")) or 0
        -- 虚拟仓位与引擎真实净仓重新同步: v_pos = Q0 + (多 − 空)
        r_long = ctx:pos_size(pair, "long") or 0
        r_short = ctx:pos_size(pair, "short") or 0
        avg_l = ctx:pos_entry(pair, "long") or 0
        avg_s = ctx:pos_entry(pair, "short") or 0
        v_pos = q0 + (r_long - r_short)
        -- 账本完整性防御: 已启动但虚拟现金缺失/非正(状态损坏) → 记账链断裂,
        -- do_rehang 量公式会退化出错误挂单量 → 必须 FATAL 停机。
        if v_cash == nil or v_cash <= 0 then
            halted = true
            fatal = 1
            save_state(ctx)
            ctx:log("[shannon_neutral_grid_futures] [FATAL] 续接失败: 已启动但状态中无有效 v_cash " ..
                "(虚拟账本不完整) -> 停机, 请人工核对持仓与保证金。")
            return
        end
        -- 重启后必须重挂(停机撤单兜底已清掉本策略挂单)
        need_rehang = true
        halted = (ctx:state_get("halted") == "1")
        if halted then
            ctx:log("[shannon_neutral_grid_futures] 续接: 上次已停机(halted=true) -> 本次不再交易")
        end
        ctx:log(string.format(
            "[shannon_neutral_grid_futures] 续接上次状态: 平衡价 %.4f, 投入 %.2f×%.1f, Q0 %.6f, " ..
            "v_cash %.2f, 真实净仓 %.6f, flag=%d",
            balance_price, invested0, leverage, q0, v_cash or 0, r_long - r_short, flag))
    end

    ctx:log(string.format(
        "[shannon_neutral_grid_futures] init pair=%s quote=%s 投入=%s 杠杆=%.1f(总资金=投入×杠杆) " ..
        "start_price=%.4f(初始平衡价, 不建仓) 主时钟=%s ATR=%s×%d mult=%.2f min_notional=%.2f " ..
        "fee_side=%.4f liq_warn=%.2f 间距放大系数=%.2f(flag 驱动, 1.0=禁用) 保证金模式=%s",
        pair, quote_asset,
        (ctx:config_f64("invest_cash") or 0) > 0 and string.format("%.2f", ctx:config_f64("invest_cash"))
            or "启动时余额全额",
        leverage, start_px,
        ctx:config_str("interval") ~= nil and ctx:config_str("interval") ~= ""
            and ctx:config_str("interval") or "1h",
        atr_intv, num(ctx, "atr_period", 14), num(ctx, "atr_mult", 1.5),
        num(ctx, "min_notional", 5), num(ctx, "fee_side", 0.0005),
        num(ctx, "liq_warn_ratio", 0.1),
        num(ctx, "flag_spacing_mult", 1.2),
        ctx:config_str("margin_mode") or "cross"))
end

-- 全撤重挂(共享决策出口): on_fill 成交后与 on_tick 重挂共用。
-- C/Q = 虚拟账本 v_cash/v_pos; 挂单为普通 one-way 限价单(不带 position_side/reduce_only,
-- 空仓卖单成交即开空); 买量不砍但做真实可用保证金预估(underfunded 可观测);
-- 两侧间距按方向 flag 指数放大; 单侧漂移死锁触发双侧纠偏重锚。
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
    local leverage = num(ctx, "leverage", 3)

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
    -- 漂移纠偏(双侧): 引擎对跳空 bar 按开盘价成交使账本逐次漂移。买侧漂移过度
    -- (C ≤ Q×买价)→ 买量出负 → 仅剩上方卖单 → 下跌行情卖单永不成交 → 平衡价冻结死锁;
    -- 卖侧镜像(C ≥ Q×卖价)→ 卖量出负 → 仅剩下方买单 → 上涨行情买单永不成交 → 同样死锁。
    -- 任一触发即把平衡价重锚到 C/Q(1:1 恒等式定义价), 两侧恢复挂单。
    -- 无漂移时不触发, "成交后平衡价 := 成交价"语义不变。
    -- ⚠ 重锚后重算挂单价必须用放大后的买/卖间距, 与实际挂单一致。
    if Q > 0 and C > 0 and (C <= Q * buy_px or C >= Q * sell_px) then
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
                    "[shannon_neutral_grid_futures] [WARN] 虚拟买单超真实可用: 需保证金 %.2f > 可用 %.2f " ..
                    "(名义 %.2f, 杠杆 %.1f) -> 照常挂出, 待引擎资金就绪成交",
                    need_cash, avail, q * buy_px, leverage))
            end
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
            -- 卖量不 cap 到持仓: 空仓/少仓卖出即开空(one-way 先平后开), 是中性策略设计意图。
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
        bump_stall((ctx:now() or {}).ts)
        if stall_since == nil then
            stall_since = (ctx:now() or {}).ts
        end
        if stall_since ~= nil and (ctx:now() or {}).ts ~= nil
            and (ctx:now()).ts - stall_since >= 86400000
            and (stall_warned_at == nil or (ctx:now()).ts - stall_warned_at >= 86400000) then
            stall_warned_at = (ctx:now()).ts
            ctx:log(string.format(
                "[shannon_neutral_grid_futures] [WARN] 停摆 ≥1 天: 平衡价 %.4f 间距 %.4f 下两侧均无法挂单 " ..
                "(虚拟现金 %.2f 虚拟持仓 %.6f, 小名义跳过 %d 次)",
                balance_price, spacing, C, Q, skip_notional))
        end
        return {}
    end

    need_rehang = false
    rehang_count = rehang_count + 1
    save_state(ctx)
    ctx:log(string.format(
        "[shannon_neutral_grid_futures] 网格重挂 #%d: 平衡价 %.4f 基础间距 %.4f (ATR %.4f×%.2f) flag=%d " ..
        "买间距 %.4f / 卖间距 %.4f -> 买 %s / 卖 %s | 虚拟现金 %.2f 虚拟持仓 %.6f 真实净仓 %.6f",
        rehang_count, balance_price, spacing, atr, atr_mult, flag, buy_spacing, sell_spacing,
        buy_px > 0 and string.format("%.4f", buy_px) or "跳过(买价≤0)",
        string.format("%.4f", sell_px),
        C, Q, (r_long or 0) - (r_short or 0)))
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
    local fee_side = num(ctx, "fee_side", 0.0005)
    local leverage = num(ctx, "leverage", 3)

    -- 真实方向仓与自维护均价每决策 bar 从引擎重同步(逐 fill 口径与引擎一致, 重同步只是
    -- 防漂移的兜底; 无仓侧均价清零)。净名义可观测: 记录运行期最大 |多−空|×价。
    r_long = ctx:pos_size(pair, "long") or 0
    r_short = ctx:pos_size(pair, "short") or 0
    avg_l = r_long > 0 and (ctx:pos_entry(pair, "long") or avg_l) or 0
    avg_s = r_short > 0 and (ctx:pos_entry(pair, "short") or avg_s) or 0
    local net_notional = math.abs(r_long - r_short) * price
    if net_notional > max_net_notional then
        max_net_notional = net_notional
    end

    -- 爆仓价监控(每决策 bar 更新): 任一方向仓距爆仓价 < liq_warn_ratio 打 WARN,
    -- 只告警不动作(去重防刷屏); 距离回升 1.5× 重置。cross 下 pos_liq 恒 nil → 自动静默。
    local min_dist = nil
    for _, side in ipairs({ "long", "short" }) do
        local sz = side == "long" and r_long or r_short
        if sz > 0 then
            local liq = ctx:pos_liq(pair, side)
            if liq and liq > 0 then
                liq_price_final = liq
                local dist = math.abs(price - liq) / price
                if min_dist == nil or dist < min_dist then
                    min_dist = dist
                end
                if liq_dist_min == nil or dist < liq_dist_min then
                    liq_dist_min = dist
                    liq_dist_min_ts = t.ts
                    liq_dist_min_price = price
                end
            end
        end
    end
    local liq_ratio = num(ctx, "liq_warn_ratio", 0.1)
    if min_dist ~= nil then
        if min_dist < liq_ratio and not liq_warned then
            liq_warned = true
            ctx:log(string.format(
                "[shannon_neutral_grid_futures] [WARN] ⚠ 逼近爆仓价: 现价 %.4f 爆仓价 %.4f 距离 %.2f%% " ..
                "(< %.2f%%) —— 合约杠杆风险, 请注意减仓或追加保证金",
                price, liq_price_final, min_dist * 100, liq_ratio * 100))
        elseif min_dist >= liq_ratio * 1.5 then
            liq_warned = false -- 距离回升: 重置, 下次再逼近再告警
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
                "[shannon_neutral_grid_futures] [WARN] 停摆 ≥1 天: ATR 未就绪已连续跳过 %d 根 bar, 不挂单",
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
    -- 死锁检测须用与实际挂单一致的放大后买/卖间距
    local buy_spacing, sell_spacing = side_spacings(spacing, flag, num(ctx, "flag_spacing_mult", 1.2))

    -- 成本门槛(启动硬校验一次): 生效间距必须 ≥ 4×单边费率(严格小于才停机)
    if not cost_checked then
        cost_checked = true
        local need = 4 * fee_side * price
        if spacing < need then
            halted = true
            fatal = 1
            save_state(ctx)
            ctx:log(string.format(
                "[shannon_neutral_grid_futures] [FATAL] 成本门槛不满足 -> 停机: 生效间距=%.6f (=%.4f%% 价格) " ..
                "必须 ≥ 4×fee_side×价格=%.6f (=%.4f%%); atr_mult=%.2f ATR=%.6f 价格=%.4f",
                spacing, spacing / price * 100, need, need / price * 100, atr_mult, atr, price))
            return {}
        end
        ctx:log(string.format(
            "[shannon_neutral_grid_futures] 成本门槛通过(首次校验): 生效间距=%.6f (%.4f%% 价格) ≥ %.4f%%",
            spacing, spacing / price * 100, 4 * fee_side * 100))
    end

    -- ── 未启动: 首个 ATR 就绪的 bar 建立虚拟账本(不建仓), 平衡价 := start_price
    if not built then
        local start_px = num(ctx, "start_price", 0)
        local inv = ctx:config_f64("invest_cash") or 0
        if inv <= 0 then
            inv = ctx:balance(quote_asset) or 0
        end
        invested0 = inv
        v_total = inv * leverage
        v_init = v_total / 2
        q0 = v_total / (2 * start_px)
        v_cash = v_total / 2
        v_pos = q0
        balance_price = start_px
        built = true
        need_rehang = true
        save_state(ctx)
        ctx:log(string.format(
            "[shannon_neutral_grid_futures] 启动(不建仓): 总资金 %.2f = 投入 %.2f × 杠杆 %.1f, " ..
            "虚拟仓位 Q0=%.6f @ 平衡价 %.4f(虚拟现金 %.2f), 真实仓位 0, 挂两侧网格单",
            v_total, invested0, leverage, q0, balance_price, v_cash))
    end

    -- 停摆恢复(此前 ATR 未就绪, 现在可以重挂了)
    if stall_since ~= nil then
        stall_since = nil
        stall_warned_at = nil
    end

    -- 死锁检测(双侧): 漂移使单侧挂单量出负 → 仅剩另一侧挂单 → 行情继续单边则永不成交
    -- → need_rehang 永假 → do_rehang 的纠偏分支永远执行不到。
    -- 按与 do_rehang 相同的触发条件强制置 need_rehang, 使纠偏生效解冻网格
    -- (买/卖价用放大后间距, 与 do_rehang 实际挂单价一致)。
    if not need_rehang and balance_price ~= nil and (v_pos or 0) > 0 and (v_cash or 0) > 0 then
        local C2, Q2 = v_cash, v_pos
        if C2 <= Q2 * (balance_price - buy_spacing)
            or C2 >= Q2 * (balance_price + sell_spacing) then
            need_rehang = true
        end
    end

    if not need_rehang then
        return {} -- 两侧单未成交 → 平衡价不变, 不重挂
    end

    return do_rehang(ctx)
end

function on_fill(ctx, fill)
    local px = fill.fill_price or balance_price or 0
    local size = fill.fill_size or 0
    local pair = ctx:config_str("pair")

    -- 爆仓判定: one-way 组合强平 fill 的 client_order_id = "LIQ-{pair}"(无侧后缀),
    -- position_side = None, side = 被平仓位方向(Buy=多头被平 / Sell=空头被平)。
    -- ⚠ 不能按 side 走 model_apply 记账(引擎已整侧清零)—— 直接以引擎仓位为真值同步。
    if (fill.client_order_id or ""):sub(1, 4) == "LIQ-" then
        liq_count = liq_count + 1
        if fill.side == "buy" then
            liq_pnl = liq_pnl + size * (px - avg_l)   -- 多头被平: 损失按多头均价
            buy_count = buy_count + 1
        else
            liq_pnl = liq_pnl + size * (avg_s - px)   -- 空头被平: 损失按空头均价
            sell_count = sell_count + 1
        end
        fill_count = fill_count + 1
        -- 账本以引擎为真值: 强平后两侧清零, v_pos 同步回 Q0; v_cash 定格(爆仓后不再交易)
        r_long, r_short, avg_l, avg_s = 0, 0, 0, 0
        v_pos = q0 + (ctx:pos_size(pair, "long") or 0) - (ctx:pos_size(pair, "short") or 0)
        -- 爆仓即停机: 持久化 halted, 重启不复活交易
        halted = true
        fatal = 1
        save_state(ctx)
        ctx:log(string.format(
            "[shannon_neutral_grid_futures] [FATAL] 💥 爆仓 -> 停机: 强平成交 %.6f @ %.4f (费 %.4f) | " ..
            "虚拟现金 %.2f 虚拟持仓 %.6f | 运行期最近爆仓距离 %s(发生时价格 %.4f) | " ..
            "已全撤在途挂单, 不再交易",
            size, px, fill.fee or 0, v_cash or 0, v_pos or 0,
            liq_dist_min and string.format("%.2f%%", liq_dist_min * 100) or "N/A",
            liq_dist_min_price))
        return { { pair = pair, action = "cancel_pending" } }
    end

    if balance_price == nil then
        return {}
    end

    -- 网格/期末清仓成交: 平衡价 := 成交价; 虚拟账本按真实成交推进。
    -- CLOSE- fill 带 position_side(被平侧), 盈亏按自维护均价单列; 不计 flag。
    local is_close = (fill.client_order_id or ""):sub(1, 6) == "CLOSE-"
    if is_close then
        if fill.position_side == "long" then
            close_pnl = close_pnl + size * (px - avg_l)
        elseif fill.position_side == "short" then
            close_pnl = close_pnl + size * (avg_s - px)
        end
    end
    balance_price = px
    model_apply(fill, px, size)
    if fill.side == "buy" then
        buy_count = buy_count + 1
    else
        sell_count = sell_count + 1
    end
    -- 方向 flag 计数: 买 −1 / 卖 +1; CLOSE- 期末强平不计(非网格主动成交); LIQ- 在上方独立处理。
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
        "真实净仓 %.6f 平衡价(新)=%.4f 1:1 检查: 虚拟现金/仓位市值 = %.4f",
        fill_count, fill.side, size, px, size * px, fill.fee or 0, flag,
        v_cash or 0, v_pos or 0, r_long - r_short, balance_price,
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
        need_rehang = true
        ctx:log(string.format(
            "[shannon_neutral_grid_futures] 挂单被拒: pair=%s filled=%.6f remaining=%.6f → 重挂",
            upd.pair, upd.filled_size or 0, upd.remaining_size or 0))
    end
end

function on_stop(ctx)
    local pair = ctx:config_str("pair")
    local pos_l = ctx:pos_size(pair, "long") or 0
    local pos_s = ctx:pos_size(pair, "short") or 0
    local net = pos_l - pos_s
    local eq = ctx:equity() or 0

    -- 对账 1: 净仓恒等式 v_pos = Q0 + 真实净仓 应逐分一致(差 > 0.01 → WARN)。
    local d_pos = (v_pos or 0) - (q0 + net)
    if math.abs(d_pos) > 0.01 then
        ctx:log(string.format(
            "[shannon_neutral_grid_futures] [WARN] 净仓分叉: 虚拟持仓 %.8f vs Q0+引擎净仓 %.8f (差 %.8f)",
            v_pos or 0, q0 + net, d_pos))
    end

    -- 对账 2: 虚拟账本重建 v_cash 应恒等于 初始虚拟现金 − Σ费 − Σ买名义 + Σ卖名义
    -- (逐 fill 递推的封闭恒等式; 再平衡浮盈合法进入 v_cash)。差 > 0.01 → WARN。
    local v_recon = v_init - m_fee - buy_notional + sell_notional
    local v_recon_diff = (v_cash or 0) - v_recon
    if v_total > 0 and math.abs(v_recon_diff) > 0.01 then
        ctx:log(string.format(
            "[shannon_neutral_grid_futures] [WARN] 虚拟账本重建核对失败: v_cash %.4f vs 重建 %.4f (差 %.4f)",
            v_cash or 0, v_recon, v_recon_diff))
    end

    -- 收益分解: 合计 = 期末权益 − 投入 = 持仓收益(净仓×(末价−净均价)) + 交易收益(再平衡净贡献)
    local out = string.format(
        "[shannon_neutral_grid_futures] 停机(**不清仓**): 成交 %d(买 %d 卖 %d) / 跳过(ATR未就绪 %d, 小名义 %d, 买价≤0 %d) / " ..
        "重挂 %d / 资金不足买 %d 次(累计名义 %.2f) / flag=%d(峰值 +%d/%d) / %s / 平衡价 %s / " ..
        "期末虚拟现金 %.2f 虚拟持仓 %.6f(Q0 %.6f) 真实净仓 %.6f(多 %.6f 空 %.6f) " ..
        "权益 %.2f / 虚拟费 %.4f / 强平锁定 %.2f / 爆仓 %d 次(损失 %.2f, 最近距离 %s) / 净仓差 %.8f",
        fill_count, buy_count, sell_count, skip_no_atr, skip_notional, skip_zero_buy_px,
        rehang_count, underfunded_buys, underfunded_notional,
        flag, flag_max, flag_min,
        halted and "已停机(成本门槛/参数/爆仓)" or "正常运行",
        balance_price and string.format("%.4f", balance_price) or "nil",
        v_cash or 0, v_pos or 0, q0, net, pos_l, pos_s, eq, m_fee, close_pnl,
        liq_count, liq_pnl,
        liq_dist_min and string.format("%.2f%%", liq_dist_min * 100) or "N/A", d_pos)
    if invested0 > 0 and last_price > 0 then
        local total = eq - invested0
        local hold = 0
        if pos_l > 0 then
            hold = hold + pos_l * (last_price - avg_l)
        end
        if pos_s > 0 then
            hold = hold + pos_s * (avg_s - last_price)
        end
        out = out .. string.format(
            " | 收益分解: 投入 %.2f × 杠杆 %.1f | 合计 %+.2f = 持仓 %+.2f + 交易(再平衡) %+.2f",
            invested0, num(ctx, "leverage", 3), total, hold, total - hold)
    end
    ctx:log(out)

    -- stat_* 导出(报告"策略统计"区块)
    ctx:state_set("stat_eff_leverage", string.format("%.1f", num(ctx, "leverage", 3)))
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
    ctx:state_set("stat_q0", string.format("%.6f", q0))
    ctx:state_set("stat_start_price", string.format("%.6f", num(ctx, "start_price", 0)))
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
    ctx:state_set("stat_net_pos_final", string.format("%.8f", net))
    ctx:state_set("stat_pos_long_final", string.format("%.8f", pos_l))
    ctx:state_set("stat_pos_short_final", string.format("%.8f", pos_s))
    ctx:state_set("stat_max_net_notional", string.format("%.2f", max_net_notional))
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
