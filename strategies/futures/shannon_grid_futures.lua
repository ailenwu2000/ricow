-- 合约香农网格策略 (shannon_grid_futures) -- v1 (2026-09-28, 040)
--
-- ═══ 一句话 ═══
--   USDT-M 合约(hedge, 只下 LONG 侧)香农网格: 逐行承袭现货 shannon_grid 的恒定权重再平衡语义 ——
--   价格低于 start_price 激活, 用**总资金(投入×杠杆)的一半**市价开多建仓; 虚拟账本(虚拟现金 v_cash +
--   虚拟仓位 v_pos)价值恒 1:1, 平衡价 ± max(atr_mult×ATR, 间距下限) 挂买/平多单, 任一成交都把
--   成交价当作新平衡价、按"成交后 1:1 恢复"的量全撤重挂两侧。
--
-- ═══ 与现货 shannon_grid 的差异(仅 6 点合约化, 其余逐行一致) ═══
--   1) 杠杆: leverage 1~5(默认 2, 清单 default_leverage 为唯一权威); 总资金 v_total = 投入×杠杆,
--      激活用 v_total/2 名义开多。⚠ 手工改杠杆必须同步 [backtest].leverage, 否则 v_total 虚增被引擎拒单。
--   2) 虚拟账本: 引入 v_cash/v_pos —— v_pos 恒等于引擎实际多头仓位(逐 fill 推进); v_cash 隔离
--      "真实可用保证金随价格/浮盈亏频繁波动"的抖动, 让网格量公式稳定。建仓后 v_cash = v_total −
--      建仓名义 − 费; 之后按现货同款 1:1 恢复公式推进(fee 扣 v_cash)。挂单量以 v_cash/v_pos 计算。
--   3) 挂单带 position_side="long": 买=开多 / 卖=平多, **绝不反向开空**; 平多量 cap 到实际持仓
--      (paired cap_close 无折扣写法, **严禁** 1e-9 比例削裁 —— 残仓致逐仓钱包 sweep 永不触发,
--      保证金滞留停摆, 032 根因 A)。
--   4) 爆仓价监控(双通道): 策略侧每决策 bar 读 ctx:pos_liq(pair,"long"), 距现价 < liq_warn_ratio
--      打 WARN(只告警不动作, 去重防刷屏); 实盘通道 = 引擎 LiqWarn 事件。爆仓价为开仓口径估算,
--      费率/资金费侵蚀后实际更近(backtest.rs 口径)。040-L: 运行期记录"距爆仓最近"时刻
--      (stat_liq_dist_min/ts/price); 引擎强平 fill(LIQ- 前缀)触发即停机(halted 持久化, 重启不复活),
--      爆仓损失单列 stat_liq_pnl(均值成本口径)。
--   5) 虚拟资金不足可观测: 买单量**不砍**; 估算占用保证金 > 真实可用现金时 WARN +
--      stat_underfunded_buys/stat_underfunded_notional 计数(引擎侧限价单保持 pending, 不阻塞)。
--   6) on_stop 对账为**合约口径**: v_pos vs 引擎持仓差 < 0.01 WARN; v_cash 与引擎现金**本就合法
--      分叉**(卖平多真实回笼 = 释放保证金 q·p/L + 已实现盈亏, 虚拟记 +q·p; 资金费引擎 8h 计收、
--      虚拟账本不计), 只留档不告警。--close-at-end 的 CLOSE- 强平 fill 按普通卖出推进虚拟账本,
--      盈亏单列 stat_close_pnl(均值成本口径)。
--   ⚠ 再平衡固有语义(同现货): 下跌后反弹的首笔平多单相对高位历史开多是保本/微亏 —— 恒定权重
--   再平衡特性, 非配对违约; 不引入 min_pair_profit/栈顶锚定(那是 paired 系语义, 香农无栈)。
--
-- ═══ 参数(全部可配, 默认值可直接回测; 清单 [[params]] 同步声明 default 注入生效配置) ═══
--   pair              必填, 合约对(如 SOLUSDT)
--   start_price       必填, 开始价格(低于它才激活); 缺失/≤0 启动停机
--   invest_cash       投入资金(保证金), 默认 0(= 激活时 quote 余额全额); 总资金 = 投入×杠杆,
--                     激活用总资金一半市价开多
--   interval          主时钟, 默认 1h(回测建议 1m 数据提升重挂/撮合粒度)
--   atr_interval      算 ATR 的 K 线周期, 默认 1h
--   atr_period        ATR 周期, 默认 14
--   atr_mult          间距 = atr_mult × ATR, 默认 1.5
--   min_spacing_pct   最小间距(占价格比例), 默认 0.002(= 0.2% = 4×合约费率 0.05%); 负数禁用下限
--   min_notional      单笔最小名义, 默认 5(合约 SOLUSDT fapi 最小名义)
--   fee_side          单边费率, 默认 0.0005(合约 taker 5bps)
--   liq_warn_ratio    距爆仓价告警阈值, 默认 0.1
--   (leverage 不进 [[params]]: 引擎保留键, 走清单 default_leverage=2 回填 [backtest].leverage;
--    Lua 仅防御性读取做 1~5 校验, 越界 FATAL 停机)
--
-- ═══ 守卫与可观测性(承袭现货) ═══
--   ATR 未就绪 → 不挂单(计数); 成本门槛: 生效间距/价格 < 4×fee_side → [FATAL] 停机
--   (0.2% 默认下限恰 = 4×0.05% 费率门槛, 往返毛利 0.1% > 0, 放行); 连续 1 天无任何挂单 → WARN。
--   期末: 虚拟权益自洽(v_cash + v_pos×末价 vs v_total − Σfee) + v_pos vs 引擎持仓对账 + stat_* 导出。
--
-- ═══ 用法 ═══
--   ricow backtest --strategy shannon_grid_futures --pair SOLUSDT --interval 1m --days 180 \
--     --cash 10000 --param start_price=<窗口起点价> --param invest_cash=10000 --close-at-end
--   ricow run shannon_grid_futures --demo   (直跑: market/position_mode/杠杆由清单回填)
--
quote_asset = ""  -- 计价资产, on_init 里 detect_quote(pair) 动态检测
-- 状态
built = false
balance_price = nil       -- 平衡价格(建仓成交价 / 最近一次网格成交价)
pending_entry = false     -- 建仓市价单在途
need_rehang = false       -- 成交/续接后置 true: 下一 tick 撤旧单 + 重挂两侧
halted = false            -- 成本门槛/参数校验不满足 → 停机
fatal = 0                 -- 数值化停机标记(单测经 global_f64 读取)
cost_checked = false      -- 成本门槛只做一次启动校验
last_bar_ts = nil
invested0 = 0             -- 投入资金(激活时刻定格; 收益分解基准)
entry_price = 0           -- 初始建仓成交价(持仓损益基准)
entry_size = 0            -- 初始建仓数量
-- 虚拟账本(合约差异 #2): v_pos = 引擎实际多头仓位; v_cash 隔离保证金波动
v_cash = nil
v_pos = nil
v_total = 0               -- 总资金 = 投入 × 杠杆(激活时刻定格)
avg_entry = 0             -- 均值开仓成本(CLOSE- 强平盈亏口径)
-- 爆仓价监控
liq_warned = false
liq_price_final = 0       -- 期末最后一次读到的爆仓价
-- 最近爆仓距离(040-L): 运行期"距爆仓最近"时刻快照, 只收紧不回退
liq_dist_min = nil        -- 最小距离 (price−liq)/price, 小数; nil = 尚无有效爆仓价
liq_dist_min_ts = nil     -- 上述最小距离发生的 bar 时间戳 ms
liq_dist_min_price = 0    -- 上述最小距离发生时的现价
liq_count = 0             -- 引擎爆仓(LIQ- fill)次数
liq_pnl = 0               -- 爆仓成交额(负值, 平多卖出名义)
-- 停摆可观测性
stall_bars = 0            -- 处于"无任何挂单"状态的主时钟 bar 数(累计, 每 bar 去重 +1)
stall_bar_ts = nil        -- 上次 stall_bars +1 的 bar 时间戳 ms(去重: 同 bar 多次路径只计一次)
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
underfunded_buys = 0      -- 虚拟买单量超真实可用保证金的次数(合约差异 #5)
underfunded_notional = 0  -- 上述买单的累计名义
close_pnl = 0             -- CLOSE- 强平锁定损益(均值成本口径)
m_fee = 0                 -- 虚拟账本累计手续费(Σ fill.fee)
buy_notional = 0          -- 虚拟账本累计买入名义(账本重建核对用)
sell_notional = 0         -- 虚拟账本累计卖出名义
last_price = 0

local function num(ctx, key, dflt)
    local v = ctx:config_f64(key)
    if v == nil or v == 0 then
        return dflt
    end
    return v
end

-- 停摆 bar 计数(去重): 同一主时钟 bar 内多条路径(on_fill 链内 do_rehang / on_tick)只计一次。
local function bump_stall(ts)
    if ts ~= nil and ts ~= stall_bar_ts then
        stall_bar_ts = ts
        stall_bars = stall_bars + 1
    end
end

-- 开多量保证金兜底(合约差异 #3, 同 paired cap_open): 名义/杠杆 ≤ 可用余额(预扣费+滑点余量)。
local function cap_open(ctx, size, p, leverage, fee_bps)
    local cash = ctx:balance(quote_asset) or 0
    local max_sz = cash * leverage / (p * (1 + (fee_bps + 20) / 10000))
    if size > max_sz then
        return max_sz
    end
    return size
end

-- 平多量持仓兜底: cap 到引擎实际多头持仓即可。**不自砍**(无 1e-9 折扣) ——
-- 残仓致 hedge 逐仓钱包 sweep 永不触发 → 保证金滞留停摆(032 根因 A, 严禁恢复)。
local function cap_close(ctx, pair, size)
    local pos = ctx:pos_size(pair, "long") or 0
    if size > pos then
        return pos
    end
    return size
end

-- 虚拟账本按真实成交推进(合约差异 #2): 与现货 model_apply 同构, 只是账本是"虚拟"的 ——
-- v_pos 与引擎逐分同步, v_cash 的引擎对应物(可用保证金)随浮盈亏波动, 故对账口径见 on_stop。
local function model_apply(fill, px, size)
    if v_cash == nil then
        return -- 首笔(建仓)成交在 on_fill 里同步初始值, 不会走到这里
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

    -- 杠杆校验(合约差异 #1): 1~5 倍, 越界 FATAL 停机。杠杆唯一权威 = 清单 default_leverage
    -- (回填 [backtest].leverage), 此处防御性读取; num() 将 0 折为默认, 下界用 <1 捕获。
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
        v_pos = ctx:pos_size(pair, "long") or 0
        -- 账本完整性防御: 已建仓但虚拟现金缺失(状态损坏/旧版本状态) → 记账链断裂,
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
            "[shannon_grid_futures] 续接上次状态: 平衡价 %.4f, 投入 %.2f×%.1f, 建仓 %.6f@%.4f, v_cash %.2f",
            balance_price, invested0, leverage, entry_size, entry_price, v_cash or 0))
    end

    ctx:log(string.format(
        "[shannon_grid_futures] init pair=%s quote=%s 投入=%s 杠杆=%.1f(总资金=投入×杠杆) 建仓=总资金一半 " ..
        "主时钟=%s ATR=%s×%d mult=%.2f min_notional=%.2f fee_side=%.4f liq_warn=%.2f",
        pair, quote_asset,
        (ctx:config_f64("invest_cash") or 0) > 0 and string.format("%.2f", ctx:config_f64("invest_cash"))
            or "激活时余额全额",
        leverage,
        ctx:config_str("interval") ~= nil and ctx:config_str("interval") ~= ""
            and ctx:config_str("interval") or "1h",
        atr_intv, num(ctx, "atr_period", 14), num(ctx, "atr_mult", 1.5),
        num(ctx, "min_notional", 5), num(ctx, "fee_side", 0.0005),
        num(ctx, "liq_warn_ratio", 0.1)))
    ctx:log(string.format(
        "[shannon_grid_futures] 激活: 价格低于 start_price=%.4f 才激活, 用总资金一半市价开多, " ..
        "成交价 = 第一平衡价格; 平衡价 ± ATR 挂开多/平多单, 成交后 1:1 恢复并重挂; " ..
        "⚠ 合约有杠杆, 请关注爆仓价告警",
        num(ctx, "start_price", 0)))
end

-- 全撤重挂(共享决策出口, 038 事件模型): on_fill 成交后与 on_tick 重挂共用。
-- 逐行承袭现货 do_rehang; 差异: C/Q = 虚拟账本 v_cash/v_pos; 挂单带 position_side="long";
-- 买量不砍但做真实可用保证金预估(underfunded 可观测); 卖量 cap_close 不自砍。
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
    -- 生效间距 = max(atr_mult×ATR, 最小间距下限×价格)(同现货口径)
    local spacing = atr_mult * atr
    local min_spacing = num(ctx, "min_spacing_pct", 0.002) * price
    if min_spacing < 0 then
        min_spacing = 0 -- 负数 = 禁用下限
    end
    if spacing < min_spacing then
        spacing = min_spacing
    end

    -- C/Q = 虚拟账本(合约差异 #2)
    local C = v_cash or 0
    local Q = v_pos or 0
    local buy_px = balance_price - spacing
    local sell_px = balance_price + spacing
    local orders = { { pair = pair, action = "cancel_pending" } }

    if buy_px > 0 then
        local q = (C - Q * buy_px) / (buy_px * (2 + f))
        if q > 0 then
            -- 合约差异 #5: 买单量**不砍**; 估算占用保证金超真实可用 → 记录可观测
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
        "[shannon_grid_futures] 网格重挂 #%d: 平衡价 %.4f 间距 %.4f (ATR %.4f×%.2f) -> 买 %s / 卖 %s | " ..
        "虚拟现金 %.2f 虚拟持仓 %.6f",
        rehang_count, balance_price, spacing, atr, atr_mult,
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

    -- 爆仓价监控(合约差异 #4, 每决策 bar 更新): 多头距爆仓价 < liq_warn_ratio 打 WARN,
    -- 只告警不动作(去重防刷屏); 距离回升 1.5× 重置。
    local pos_now = ctx:pos_size(pair, "long") or 0
    if pos_now > 0 then
        local liq = ctx:pos_liq(pair, "long")
        if liq and liq > 0 then
            liq_price_final = liq
            -- 最近爆仓距离追踪(040-L): 只收紧不回退
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

    -- 成本门槛(启动硬校验一次, 同现货 4×fee 口径): 生效间距必须 ≥ 4×单边费率(严格小于才停机;
    -- 默认 0.2% 下限恰等门槛, 往返毛利 0.1% > 0, 放行)
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

    if not need_rehang then
        return {} -- 两侧单未成交 → 平衡价不变, 不重挂
    end

    -- 重挂出口(共享决策出口, 038 事件模型): 全撤重挂逻辑已抽至 do_rehang, 语义不变
    return do_rehang(ctx)
end

function on_fill(ctx, fill)
    -- 只做多: 非 long 侧成交忽略(防御, 正常不应出现)
    if fill.position_side and fill.position_side ~= "long" then
        return {}
    end
    local px = fill.fill_price or balance_price or 0
    local size = fill.fill_size or 0
    local pair = ctx:config_str("pair")

    -- 爆仓判定(040-L): 引擎强平 fill 的 client_order_id 以 "LIQ-" 开头
    -- (hedge: LIQ-{pair}-long; one-way: LIQ-{pair})。⚠ 注意引擎强平 fill 的 side 语义是
    -- "追加该方向"口径(平多报 buy), 不能按 side 走 model_apply —— 直接以引擎仓位为真值同步。
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
        -- 建仓成交: 该笔成交价 = 第一平衡价格; 虚拟账本在此初始化:
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
            "[shannon_grid_futures] 建仓成交 %.6f @ %.4f (费 %.4f) -> 平衡价 := %.4f | " ..
            "虚拟现金 %.2f 虚拟持仓 %.6f 仓位市值 %.2f (总资金 %.2f = 投入 %.2f × 杠杆)",
            size, px, fill.fee or 0, balance_price, v_cash or 0, v_pos or 0, (v_pos or 0) * px,
            v_total, invested0))
        -- 038 事件模型: 建仓成交立即全撤重挂(do_rehang)。链内限价单由引擎 038 R1 次 bar 生效,
        -- 无同 bar 乒乓链; 撤单指令即时生效。
        if halted then
            return {}
        end
        return do_rehang(ctx)
    end

    -- 网格成交: 平衡价 := 成交价; 虚拟账本按真实成交推进; CLOSE- 强平单盈亏单列(均值成本口径)
    if fill.side == "sell" and (fill.client_order_id or ""):sub(1, 6) == "CLOSE-" then
        close_pnl = close_pnl + size * (px - avg_entry)
    end
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
        "[FILL] #%d %s size=%.6f px=%.4f notional=%.2f 费=%.4f | 虚拟现金 %.2f 虚拟持仓 %.6f " ..
        "平衡价(新)=%.4f 1:1 检查: 虚拟现金/仓位市值 = %.4f",
        fill_count, fill.side, size, px, size * px, fill.fee or 0,
        v_cash or 0, v_pos or 0, balance_price,
        (v_pos and v_pos > 0 and v_pos * px > 0) and (v_cash or 0) / (v_pos * px) or 0))
    -- 038 事件模型: 网格成交立即全撤重挂(平衡价 := 本笔成交价)。链内限价单由引擎 038 R1
    -- 次 bar 生效, 无同 bar 乒乓链; 撤单指令即时生效。
    -- 停机防御(038 + 040-L): halted 后**仍记账**(保持 v_pos 与引擎逐分同步)但不再产出订单。
    if halted then
        return {}
    end
    return do_rehang(ctx)
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
            "[shannon_grid_futures] 挂单被拒: pair=%s filled=%.6f remaining=%.6f → 重挂",
            upd.pair, upd.filled_size or 0, upd.remaining_size or 0))
    end
end

function on_stop(ctx)
    local pair = ctx:config_str("pair")
    local pos = ctx:pos_size(pair, "long") or 0
    local eq = ctx:equity() or 0

    -- 对账(合约差异 #6, 合约口径):
    --   v_pos vs 引擎持仓: 应逐分一致, 差 > 0.01 → WARN;
    --   v_cash vs 引擎现金: **本就合法分叉**(卖平多真实回笼 = 释放保证金 q·p/L + 已实现盈亏,
    --   虚拟记 +q·p; 资金费引擎 8h 计收、虚拟账本不计), 只留档不告警。
    local d_pos = (v_pos or pos) - pos
    if math.abs(d_pos) > 0.01 then
        ctx:log(string.format(
            "[shannon_grid_futures] [WARN] 持仓分叉: 虚拟持仓 %.8f vs 引擎 %.8f (差 %.8f)",
            v_pos or 0, pos, d_pos))
    end

    -- 虚拟账本重建核对: v_cash 应恒等于 总资金 − Σ费 − Σ买名义 + Σ卖名义
    -- (逐 fill 递推的封闭恒等式; Shannon 再平衡浮盈合法进入 v_cash, 故不能用
    --  "v_cash+v_pos×价 vs 总资金" 这类不含盈亏项的伪恒等式)。差 > 0.01 → WARN。
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
        "重挂 %d / 资金不足买 %d 次(累计名义 %.2f) / %s / 平衡价 %s / 期末虚拟现金 %.2f 持仓 %.6f(市值 %.2f) " ..
        "权益 %.2f / 虚拟费 %.4f / 强平锁定 %.2f / 爆仓 %d 次(损失 %.2f, 最近距离 %s) / 持仓差 %.8f",
        fill_count, buy_count, sell_count, skip_no_atr, skip_notional, skip_zero_buy_px,
        rehang_count, underfunded_buys, underfunded_notional,
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

    -- 规范 §C.1: stat_* 导出(报告"策略统计"区块)
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
    -- 040-L 爆仓风险可观测: 运行期最近爆仓距离 + 爆仓事件
    ctx:state_set("stat_liq_dist_min",
        liq_dist_min and string.format("%.6f", liq_dist_min * 100) or "")
    ctx:state_set("stat_liq_dist_min_ts",
        liq_dist_min_ts and string.format("%d", liq_dist_min_ts) or "")
    ctx:state_set("stat_liq_dist_min_price", string.format("%.6f", liq_dist_min_price))
    ctx:state_set("stat_liq_count", string.format("%d", liq_count))
    ctx:state_set("stat_liq_pnl", string.format("%.4f", liq_pnl))
    ctx:state_set("stat_halted", halted and "1" or "0")
end
