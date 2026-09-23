-- 香农现货网格 (shannon_spot_grid) -- v5 (2026-09-23, 030 定稿: 纯网格挂单 + 趋势门控)
--
-- ═══ 一句话 ═══
--   用「虚拟账本」定义应该持多少币: 在平衡价上下 atr_mult x ATR 各挂一张限价单, 任一成交就把
--   成交价当作新的平衡价并立即重挂两侧。趋势门控决定何时允许买、何时允许卖。
--
-- ═══ 策略含义(用户 2026-09-23 口径, 逐条) ═══
--   1) 建仓(必须): 给 start_price(开始价格); 价格**低于**它才激活。
--      可选 initial_buy_amount: 激活时市价买入该金额作为初始仓位 -> **该笔成交价 = 第一次平衡价**。
--      不给 initial_buy_amount 时, 平衡价 = 激活瞬间的现价(不建仓)。
--   2) 虚拟账本: 激活时 v_cap = real_cash x leverage_mult, 按平衡价 50:50 分为 v_coin / v_cash。
--      之后 v_coin / v_cash **按真实成交逐笔加减**(买 -> 币增钱减, 卖 -> 币减钱增);
--      v_cap = v_coin x 现价 + v_cash 为**动态值**(随价格与买卖变化)。
--   3) 挂单: 平衡价 - atr_mult x ATR 挂买单, 平衡价 + atr_mult x ATR 挂卖单。
--      挂单量 = 让虚拟账本在该挂单价上回到 target_ratio 权重所需的小额增量:
--        买单 q = (v_cash - v_coin x p) / (2 x p)     [保证成交后 50:50 @p]
--        卖单 q = (v_coin x p - v_cash) / (2 x p)
--      卖出量另有上限: 不超过真实持仓(不足就有多少挂多少, 不重算虚拟仓位)。
--   4) 成交: 平衡价 := 该笔成交价; 虚拟账本按真实成交加减; 立即重挂两侧(撤旧单 + 挂新单)。
--   5) 结束: 实际仓位被卖空 -> 策略结束(不再动作)。停机**不清仓**。
--
-- ═══ 收益分解(停机时自动打印) ═══
--   合计收益 = 期末权益 - 投入(投入 = 激活时刻的账户权益)
--   持仓收益 = 期末持仓量 x (末价 - 初始建仓价)   ← 初始仓吃到/吐回的涨幅
--   交易收益 = 合计收益 - 持仓收益               ← 网格低买高卖的净贡献(含手续费)
--   实证(SOL 180d/15m/ATR2/杠杆2/半仓): +3956 = 持仓 +2777 + 交易 +1179。
--   杠杆的作用是**在两者之间做分配**(放大挂单量 -> 交易收益升、持仓收益降, 总收益几乎不变)。
--
-- ═══ 适用范围(实测口径) ═══
--   适合: 震荡市(双向收割最有效)、温和上涨。
--   不适合: 单边急涨 -- 网格在上涨中持续卖出, 会跑输「同敞口买入持有」;
--           单边下跌 -- 账本持续要求买入, 库存被填满、浮亏放大。
--   实测(SOLUSDT / 15m / ATR2 / 杠杆2 / 初始半仓 / 180 天, 标的 +40.7%):
--     策略 +20.07%, 最大回撤 20.45%;  满仓持有 +39.5%, 回撤 38.3%。
--     差距主要来自**敞口只有一半**, 而非策略亏损; 对「同敞口买入持有」的净贡献约 +0.3 ~ +0.8pp。
--   数学定位: 正期望来自标的漂移 beta, 策略自身只贡献「收割 - 摩擦」, 不创造收益。
--
-- ═══ 参数(全部可配, 默认值可直接回测) ═══
--   pair              必填, 交易对(现货)
--   start_price       必填, 开始价格(低于它才激活)
--   interval          主时钟, 默认 1h(回测常用 15m)
--   atr_interval      算 ATR 的周期, 默认 1h; atr_period 默认 14
--   atr_mult          网格间距 = atr_mult x ATR, 默认 2
--   real_cash         本金(虚拟账本基准), 缺省 = 激活时刻的账户权益(即 --cash 本金)
--   initial_buy_amount  初始建仓金额, 默认 0(=不建仓); 建议 = 本金的一半
--   leverage_mult     虚拟杠杆, 默认 2(钳制 1~5)
--   target_ratio      虚拟账本的币权重, 默认 0.5
--   trend_gate        趋势门控开关, 默认 off(=纯网格); on = BULL 暂停卖出 / BEAR 暂停买入
--   regime_filter     判据类型, 默认 ema200(off 关闭)
--   regime_interval   判据周期, 默认 1h;  regime_ema_period 默认 200;  regime_band_pct 默认 0.03
--   min_notional      单笔最小名义, 默认 5
--   fee_side          单边费率, 默认 0.001(现货 0.1%/边)
--
-- ═══ 如何调整(每项的效果与取舍) ═══
--   atr_mult(间距)     越大 -> 交易越少、单次利润越厚、越抗噪; 越小 -> 收割越频繁但费损吃掉利润。
--                      硬门槛: atr_mult x ATR / 价格 必须 > 4 x fee_side(= 0.4%), 不满足直接 [FATAL] 停机。
--   leverage_mult      放大的是**每笔挂单量**, 不是收益。实测 2->5 总收益几乎不变(收益来源从
--                      「持仓」转向「交易」), 但持仓衰减更快、回撤略升。纯现货下建议 2。
--   trend_gate         默认 off(纯网格, 不加任何趋势判断)。设为 on 时: 上涨趋势中暂停卖出
--                      -> 仓位不再被逐渐卖空、回撤更低, 但成交变少、交易收益下降(实测总收益基本持平,
--                      180 天 SOL 差异 < 0.3pp, 故默认关闭)。
--   regime_band_pct    门控灵敏度: 0.03 = 价格需超 EMA200 的 3% 才算 BULL(门控更少触发);
--                      调小(如 0.01)门控更频繁, 更贴近「价格在 EMA200 上方就不卖」。
--   initial_buy_amount 决定初始敞口: 给本金一半 = 半仓起步; 给本金全额 = 满仓起步(更接近买入持有)。
--   interval           主时钟越短 -> 网格越密、成交越多, 但每笔利润越薄、手续费占比越高。
--
-- ═══ 规则编号(对应 030 规格文档) ═══
--   R1 主时钟 = 信号/网格周期, 收盘后立即决策; R2 建仓(start_price + initial_buy_amount);
--   R3 虚拟账本初始化; R4 网格挂单与重挂; R5 成交后平衡价 := 成交价 + 账本按真实成交加减;
--   R6 杠杆钳制 1~5; R7 成本门槛硬校验(一次性, 不满足停机); R8 状态持久化(断点续接);
--   趋势门控(BULL 拦卖 / BEAR 拦买, 由 trend_gate 开关)。
--
-- ═══ 守卫与停机(不满足就跳过并计数, 绝不硬下单) ═══
--   ATR 未就绪 / 判据未就绪 -> 不开新单; 单笔名义 < min_notional -> 该侧不挂;
--   买量受真实现金兜底(预扣手续费); 卖量受真实持仓兜底; R7 不满足 -> 报错停机;
--   仓位清空 -> 策略结束。引擎自动注入上次平衡价与账本(表 strategy_state), 重启续接。
--
-- ═══ 用法 ═══
--   ricow backtest --strategy shannon_spot_grid --pair SOLUSDT --interval 15m --days 180 --cash 20000 \
--     --param start_price=1000 --param initial_buy_amount=10000 --param atr_interval=15m \
--     --param atr_mult=2 --param real_cash=20000 --param leverage_mult=2 --param trend_gate=on
--   TOML: type = "shannon_spot_grid" + [strategy.params] 填上面参数表。
--
quote_asset = "USDT"
-- 虚拟账本(R6: 虚拟资金 = 投入资金 x 杠杆)
v_cap = 0
v_coin = 0
v_cash = 0
-- 状态
built = false
balance_price = nil
last_side = nil
last_bar_ts = nil
prev_fast = nil
prev_slow = nil
finished = false        -- 实际仓位清空 -> 策略结束(用户 2026-09-23)
halted = false          -- R7 成本门槛不满足 -> 停机(不再下单)
cost_checked = false    -- R7 只做一次启动校验(030: 不再每 tick 复查)
-- 网格挂单(030-A)
need_rehang = false      -- 成交后置 true: 下一 tick 撤旧单 + 按新平衡价重挂两侧
pending_entry = false    -- 初始建仓市价单已发出, 等成交回调
entry_price = 0          -- 初始建仓成交价(收益分解的持仓成本基准)
entry_size = 0           -- 初始建仓数量
invested0 = 0            -- 激活时刻的账户权益(= 投入本金, 收益分解基准)
rehang_count = 0
grid_buy_px = 0
grid_sell_px = 0
last_price = 0
-- 计数(可观测性)
fill_count = 0
buy_count = 0
sell_count = 0
buy_capped = 0          -- R4 买入被现金截断的次数
skip_no_atr = 0
skip_notional = 0
skip_signal = 0         -- 被趋势判据拦下的信号次数
-- 趋势判据(1h EMA200)(默认开): BULL 不卖 / BEAR 不买 / RANGE 正常
regime_state = "?"
regime_switch = 0
regime_block_buy = 0
regime_block_sell = 0
regime_ready_skip = 0
ticks_bull = 0
ticks_range = 0
ticks_bear = 0

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


-- 趋势判据(用户 2026-09-18 定稿; 2026-09-23 周期由日线改为 1h EMA200):
--   BULL : `close >  EMA200 × (1 + band)`  → 可以买, **不卖**
--   BEAR : `close <  EMA200 × (1 − band)`  → 可以卖, **不买**
--   RANGE: `EMA200 × (1 − band) ≤ close ≤ EMA200 × (1 + band)`(含等号) → 正常(两侧都放开)
-- 判据输入 = **上一根已收盘**高周期(默认 1h)bar 的 close 与其 EMA —— 引擎侧按桶缓存,
-- 日内不变且无前视。通道未装 / 可见 bar 不足 period 根 → 返回 nil(未就绪, 由调用方决定不下单)。
local function regime_of(ctx)
    local pair = ctx:config_str("pair")
    local close = ctx:close_tf(pair)
    local ema = ctx:ema_tf(pair)
    if not close or not ema or ema <= 0 then
        return nil
    end
    local band = num(ctx, "regime_band_pct", 0.03)
    local upper = ema * (1 + band)
    local lower = ema * (1 - band)
    local state = "range"
    if close > upper then
        state = "bull"
    elseif close < lower then
        state = "bear"
    end
    if state ~= regime_state then
        regime_switch = regime_switch + 1
        regime_state = state
        ctx:log(string.format(
            "[shannon_spot_grid] 趋势判据(1h EMA200) → %s (第 %d 次切换): close=%.4f ema=%.0f %.4f " ..
            "带 [%.4f, %.4f]%s",
            string.upper(state), regime_switch, close, num(ctx, "regime_ema_period", 200),
            ema, lower, upper,
            state == "bull" and " → 只买不卖" or (state == "bear" and " → 只卖不买" or " → 两侧正常")))
    end
    return state
end

local function target_ratio_of(ctx)
    return num(ctx, "target_ratio", 0.5)
end

-- 账本名义规模 = 倍数 x 基准。
--   leverage_basis = "cash"(默认, 现状): 基准 = 配置 real_cash(初始本金) -> 规模恒定,
--       账户复利增长会摊薄有效倍数(赚钱时降杠杆), 亏损时反而放大(见文档 §十八)。
--   leverage_basis = "equity": 基准 = 账户**当前权益** -> 倍数恒定, 赢时加仓、亏时自动缩仓。
local function leverage_basis(ctx)
    local b = cfg_str(ctx, "leverage_basis")
    if b == nil or b == "" then
        return "cash"
    end
    return b
end

local function virtual_cap(ctx)
    -- R6: 杠杆倍数钳制到 1~5(用户第 7 条: 默认 2, 可设 1~5)
    local mult = num(ctx, "leverage_mult", 2)
    if mult < 1 then
        mult = 1
    end
    if mult > 5 then
        mult = 5
    end
    if leverage_basis(ctx) == "equity" then
        local eq = ctx:equity() or 0
        if eq > 0 then
            return eq * mult
        end
    end
    -- real_cash 未显式给定时 -> 用账户权益(本金), 避免"用户设了 --cash 20000 但账本按默认 1 万算"的陷阱
    local r = num(ctx, "real_cash", 0)
    if r <= 0 then
        r = ctx:equity() or 0
    end
    if r <= 0 then
        r = 10000
    end
    return r * mult
end

-- 恒定杠杆: 每 tick 把账本按"名义规模变化"等比缩放(保持币/现金权重, 等价于账本继续持有,
-- 绝对值(沙箱不含 math 库, 手写)
local function fabs(x)
    if x < 0 then
        return -x
    end
    return x
end

-- Kaufman 效率比 ER = |close_t - close_{t-n}| / 总路径 (主序列尾窗, 无前视)。
-- 实测(SOL 4h 2022-2025): 趋势期中位 0.27~0.32, 震荡期 0.14~0.16 —— 分离度优于 ADX/均线组合。


-- I1: 把虚拟账本整本重算成"10 万本金在参考价 P_ref 处的 target 权重快照"。
local function v_reset(ctx, p_ref)
    v_cap = virtual_cap(ctx)
    local target = target_ratio_of(ctx)
    v_coin = target * v_cap / p_ref
    v_cash = (1 - target) * v_cap
    return v_coin
end

-- 下单量 = |目标持仓 − 真实持仓|(用户 2026-09-22 逐字口径: "按照上次平衡价格的仓位,
-- 计算当前应该买入多少, 才能保持平衡")。
-- ⚠️ 口径变更史: 2026-09-18 曾纠正为"只看虚拟账本自己的回平衡量"; 2026-09-22 用户重新拍板
-- 回到字面口径(见 030 规格 §三 R3/R4/R5) —— 两种口径的回测数字都在 030 变更档案里留档对比。
local function book_rebalance(ctx, side, p)
    local gap = v_cash - v_coin * p
    if side == "buy" then
        if gap <= 0 then return 0 end
        return gap / (2 * p)
    end
    if gap >= 0 then return 0 end
    return -gap / (2 * p)
end

-- 下单量 = **虚拟账本回平衡量本身**(不做任何比例缩放)。
-- 用户口径(2026-09-18): "完全按虚拟仓位计算应该买入多少, 跟实际仓位没有任何关系" ——
-- 账户只有 1 万而账本是 10 万, 所以钱会用完; 资金不足只表现为**截断/跳过**(有计数),
-- 绝不用比例偷偷缩小下单量(那等于把用户要的放大效果取消掉)。
local function cap_size(ctx, side, size, p, fee_bps)
    local pair = ctx:config_str("pair")
    if side == "buy" then
        local cash = ctx:balance(quote_asset) or 0
        -- 余量 = 手续费 + 滑点(引擎市价单按 bar.open×(1±slippage) 成交) + f64 精度。
        -- 只留手续费不够: 顶满现金的市价单会被滑点推过余额线 → 引擎直接拒单(fill 恒 0)。
        local max_buy = cash / (p * (1 + (fee_bps + 20) / 10000))
        if size > max_buy then
            return max_buy
        end
        return size
    end
    -- Lua 只有 f64: 直接取"满仓"会被精度反咬 —— 例如持仓 0.7974481658692142982 经 f64 往返
    -- 变成 0.7974481658692144,**比真实持仓略大** → 引擎按"无仓可卖/超卖"处理, 卖单永远不成交
    -- (2026-09-18 实测: 网格卖单反复挂出但 0 成交)。故卖量留 1e-9 安全折扣。
    local pos = (ctx:position_size(pair) or 0) * (1 - 1e-9)
    if size > pos then
        return pos
    end
    return size
end

local function log_state(ctx, tag)
    local pair = ctx:config_str("pair")
    local price = ctx:price(pair) or 0
    local pos = ctx:position_size(pair) or 0
    local cash = ctx:balance(quote_asset) or 0
    ctx:log(string.format(
        "[shannon_spot_grid] %s 持仓 %.6f 权益 %.2f 现金 %.2f 锚 %s v_coin %.6f",
        tag, pos, cash + pos * price, cash,
        balance_price and string.format("%.4f", balance_price) or "nil", v_coin))
end

-- R8 状态持久化: 平衡价与账本写入引擎状态表(引擎在成交后与停机时落库, 启动时自动注回)。
function save_state(ctx)
    if balance_price ~= nil then
        ctx:state_set("balance_price", string.format("%.10f", balance_price))
    end
    ctx:state_set("v_coin", string.format("%.10f", v_coin))
    ctx:state_set("v_cash", string.format("%.10f", v_cash))
    ctx:state_set("v_cap", string.format("%.4f", v_cap))
    ctx:state_set("built", built and "1" or "0")
    ctx:state_set("entry_price", string.format("%.10f", entry_price))
    ctx:state_set("entry_size", string.format("%.10f", entry_size))
    ctx:state_set("invested0", string.format("%.2f", invested0))
end

function on_init(ctx)
    local pair = ctx:config_str("pair")
    quote_asset = exec.detect_quote(pair)
    balance_price = nil
    last_bar_ts = nil
    prev_fast = nil
    prev_slow = nil
    regime_state = "?"
    halted = false
    v_cap = virtual_cap(ctx)
    -- R8 断点续接: 引擎已把上次会话的状态注入(见引擎的 strategy_state 表)。
    local bp = ctx:state_get("balance_price")
    if bp ~= nil and tonumber(bp) and tonumber(bp) > 0 then
        balance_price = tonumber(bp)
        built = true
        v_coin = tonumber(ctx:state_get("v_coin")) or 0
        v_cash = tonumber(ctx:state_get("v_cash")) or 0
        v_cap = tonumber(ctx:state_get("v_cap")) or v_cap
        entry_price = tonumber(ctx:state_get("entry_price")) or 0
        entry_size = tonumber(ctx:state_get("entry_size")) or 0
        invested0 = tonumber(ctx:state_get("invested0")) or 0
            ctx:log(string.format(
            "[shannon_spot_grid] R8 续接上次状态: 平衡价 %.4f, 账本(币 %.6f 现金 %.2f 规模 %.0f)",
            balance_price, v_coin, v_cash, v_cap))
    end
    local lev = num(ctx, "leverage_mult", 2)
    if lev < 1 then
        lev = 1
    end
    if lev > 5 then
        lev = 5
    end
    ctx:log(string.format(
        "[shannon_spot_grid] init pair=%s quote=%s 杠杆=%.1f(钳制 1~5) 基准=%s 虚拟资金=%.0f " ..
        "(real_cash=%.0f) target=%.2f atr=%s×%d mult=%.2f fee_side=%.4f",
        pair, quote_asset, lev, leverage_basis(ctx), v_cap, num(ctx, "real_cash", 10000),
        target_ratio_of(ctx), cfg_str(ctx, "atr_interval", "1h"), num(ctx, "atr_period", 14),
        num(ctx, "atr_mult", 2), num(ctx, "min_notional", 5), num(ctx, "fee_side", 0.001),
        cfg_str(ctx, "trend_gate", "on")))

    local init_amt = num(ctx, "initial_buy_amount", 0)
    if init_amt > 0 then
        ctx:log(string.format(
            "[shannon_spot_grid] 建仓: 价格低于 start_price=%.4f 才激活, 并市价买入 %.2f %s 作为初始仓位(该笔成交价 = 第一次平衡价)",
            num(ctx, "start_price", 0), init_amt, quote_asset))
    else
        ctx:log(string.format(
            "[shannon_spot_grid] 建仓: 价格低于 start_price=%.4f 才激活, 不建初始仓(平衡价 = 激活瞬间现价)",
            num(ctx, "start_price", 0)))
    end
    if cfg_str(ctx, "regime_filter", "ema200") == "ema200" then
        ctx:log(string.format(
            "[shannon_spot_grid] 趋势判据(1h EMA200): ema200 interval=%s ema=%d band=%.3f " ..
            "(BULL 只买不卖 / BEAR 只卖不买 / RANGE 两侧正常; 判据未就绪不下单)",
            cfg_str(ctx, "regime_interval", "1h"), num(ctx, "regime_ema_period", 200),
            num(ctx, "regime_band_pct", 0.03)))
    else
        ctx:log("[shannon_spot_grid] 趋势判据(1h EMA200): 已关闭(regime_filter=off)")
    end
end

function on_tick(ctx)
    if finished then
        return {}
    end
    local pair = ctx:config_str("pair")
    local atr_mult = num(ctx, "atr_mult", 2)
    local min_notional = num(ctx, "min_notional", 5)
    -- R7: 单边费率(现货 0.1%/边 → 0.001); fee_bps = 万分之一单位(供 cap_size 的余量计算)
    local fee_side = num(ctx, "fee_side", 0.001)
    local fee_bps = fee_side * 10000
    -- 趋势判据(1h EMA200): ema200(默认开) / off(关闭)
    local regime_filter = cfg_str(ctx, "regime_filter", "ema200")

    -- ── 1. 决策节流: 只在主时钟收线时决策一次(主时钟 = 网格周期)。
    local k = ctx:klines(pair)
    local ts = nil
    if k and #k > 0 then
        ts = k[#k].ts
    end
    local new_bar = false
    if ts ~= nil then
        new_bar = (ts ~= last_bar_ts)
        if not new_bar then
            return {}
        end
        last_bar_ts = ts
    else
        return {}
    end

    local price = ctx:price(pair)
    -- 停机日志需要: 末次价格 + 账户当前权益(用于算"有效杠杆")
    last_price = price or last_price
    if not price or price <= 0 then
        return {}
    end

    -- ── 2. 高周期 ATR(引擎按桶缓存, 每根高周期 bar 只算一次); 通道未就绪不下单
    local atr = ctx:atr_tf(pair)
    if not atr or atr <= 0 then
        skip_no_atr = skip_no_atr + 1
        if skip_no_atr == 1 or skip_no_atr % 240 == 0 then
            ctx:log(string.format(
                "[shannon_spot_grid] 高周期 ATR 未就绪(跳过 %d 次): interval=%s period=%d",
                skip_no_atr, cfg_str(ctx, "atr_interval", "1h"), num(ctx, "atr_period", 14)))
        end
        return {}
    end
    local spacing = atr_mult * atr

    -- ── 3. R7 成本门槛(**启动硬校验**, 用户 2026-09-22): 只在首次拿到 ATR 时校验一次 --
    --    触发偏离 x = atr_mult×ATR/价格 必须 > 4×单边费率(现货 0.1%/边 -> 0.4%), 否则每次往返净亏。
    --    不满足 -> 打印 [FATAL] 并**停机**; 已停机则直接短路(不重复刷日志)。
    if halted then
        return {}
    end
    if not cost_checked then
        cost_checked = true
        local need = 4 * fee_side * price
        if spacing <= need then
            halted = true
            ctx:log(string.format(
                "[shannon_spot_grid] [FATAL] R7 成本门槛不满足 -> 停机: atr_mult×ATR=%.6f " ..
                "(=%.4f%% 价格) 必须 > 4×fee_side×价格=%.6f (=%.4f%%); atr_mult=%.2f ATR=%.6f " ..
                "价格=%.4f fee_side=%.4f。请增大 atr_mult / atr_interval, 或改用波动更大的标的。",
                spacing, spacing / price * 100, need, need / price * 100,
                atr_mult, atr, price, fee_side))
            return {}
        end
        ctx:log(string.format(
            "[shannon_spot_grid] R7 成本门槛通过(首次校验): atr_mult×ATR=%.6f (%.4f%% 价格) > %.4f%%",
            spacing, spacing / price * 100, 4 * fee_side * 100))
    end

    -- 虚拟仓位总资产 = 币 × 现价 + 现金(**动态**: 随价格与买卖变化; 用户 2026-09-23 口径)
    if built then
        v_cap = v_coin * price + v_cash
    end

    -- ── 3.5 趋势判据(1h EMA200)(默认开): BULL 拦卖 / BEAR 拦买 / RANGE 两侧正常。
    --    判据未就绪(通道未装 / 可见高周期 bar 不足 period 根)→ **不下单**(与 ATR 未就绪同口径, 不猜值)。
    local block_buy, block_sell = false, false
    if regime_filter == "ema200" then
        local state = regime_of(ctx)
        if state == nil then
            regime_ready_skip = regime_ready_skip + 1
            if regime_ready_skip == 1 or regime_ready_skip % 240 == 0 then
                ctx:log(string.format(
                    "[shannon_spot_grid] 趋势判据(1h EMA200)未就绪(跳过 %d 次): interval=%s ema=%d → 不下单",
                    regime_ready_skip, cfg_str(ctx, "regime_interval", "1h"),
                    num(ctx, "regime_ema_period", 200)))
            end
            return {}
        end
        if state == "bull" then
            ticks_bull = ticks_bull + 1
            block_sell = true
        elseif state == "bear" then
            ticks_bear = ticks_bear + 1
            block_buy = true
        else
            ticks_range = ticks_range + 1
        end
    end

    -- ── 5. 未激活: 必须设 `start_price`(开始价格), 价格低于它才激活(2026-09-23 用户口径)
    if not built then
        if pending_entry then
            return {} -- 初始建仓市价单已发出, 等成交回调
        end
        local start_px = num(ctx, "start_price", 0)
        if start_px <= 0 then
            halted = true
            ctx:log("[shannon_spot_grid] [FATAL] 缺少必填参数 start_price(开始价格) -> 停机: " ..
                "策略只在价格低于 start_price 时激活, 无此参数无法工作。")
            return {}
        end
        if price >= start_px then
            return {}
        end
        -- 激活: 设了初始仓位金额 -> 市价买入, 成交价即第一次平衡价格; 未设 -> 平衡价 = 现价
        local amt = num(ctx, "initial_buy_amount", 0)
        if amt > 0 then
            local size = cap_size(ctx, "buy", amt / price, price, fee_bps)
            if size > 0 and size * price >= min_notional then
                pending_entry = true
                ctx:log(string.format(
                    "[shannon_spot_grid] 激活(价 %.4f < 开始价 %.4f) -> 市价买入初始仓位 %.6f (≈%.2f %s), " ..
                    "成交价将成为第一次平衡价格", price, start_px, size, size * price, quote_asset))
                return {
                    { pair = pair, action = "cancel_pending" },
                    { pair = pair, side = "buy", size = size, order_type = "market" },
                }
            end
            ctx:log(string.format(
                "[shannon_spot_grid] 激活但初始仓位名义 %.2f < min_notional %.2f -> 按无初始仓处理",
                size * price, min_notional))
        end
        built = true
        balance_price = price
        v_reset(ctx, balance_price)
        need_rehang = true
        save_state(ctx)
        ctx:log(string.format(
            "[shannon_spot_grid] 激活(价 %.4f < 开始价 %.4f), 未设初始仓位 -> 平衡价 := 现价 %.4f, 进入网格",
            price, start_px, balance_price))
        return {}
    end

    -- ── 6. 已激活: 网格挂单(**唯一进出通道**, 2026-09-23 用户口径)
    --    平衡价 − atr_mult×ATR 挂买单; 平衡价 + atr_mult×ATR 挂卖单;
    --    任一成交后平衡价 := 该笔成交价, 并在下一 tick 撤旧单重挂两侧。
    if balance_price == nil then
        return {}
    end
    if not need_rehang then
        return {} -- 两侧单未成交 → 平衡价不变, 不重挂(避免频繁撤挂)
    end
    local buy_px = balance_price - spacing
    local sell_px = balance_price + spacing
    if buy_px <= 0 then
        return {}
    end
    local real = ctx:position_size(pair) or 0
    -- 挂单量 = 账本在该挂单价回到 target 权重的小额增量(与真实持仓无关, 见 book_rebalance)
    local buy_size = cap_size(ctx, "buy", book_rebalance(ctx, "buy", buy_px), buy_px, fee_bps)
    local sell_size = book_rebalance(ctx, "sell", sell_px)
    -- 卖出量受真实持仓限制(用户 2026-09-23: 不足就有多少挂多少, 不重算虚拟仓位)
    if sell_size > real then
        sell_size = real
    end

    -- 趋势门控(方案 B / 用户 2026-09-23; 可选可配置: trend_gate = on|off, 默认 on)
    --   BULL  -> 暂停卖出(让仓位随趋势漂移, 保住趋势收益)
    --   BEAR  -> 暂停买入
    --   RANGE -> 两侧正常
    local gate_blocked = false
    if cfg_str(ctx, "trend_gate", "off") == "on" then
        if block_sell and sell_size > 0 then            -- BULL: 暂停卖出(让仓位随趋势漂移)
            sell_size = 0
            regime_block_sell = regime_block_sell + 1
            gate_blocked = true
        end
        if block_buy and buy_size > 0 then              -- BEAR: 暂停买入
            buy_size = 0
            regime_block_buy = regime_block_buy + 1
            gate_blocked = true
        end
    end
    local orders = { { pair = pair, action = "cancel_pending" } }
    if buy_size > 0 and buy_size * buy_px >= min_notional then
        orders[#orders + 1] = { pair = pair, side = "buy", size = buy_size, order_type = "limit", price = buy_px }
    end
    if sell_size > 0 and sell_size * sell_px >= min_notional then
        orders[#orders + 1] = { pair = pair, side = "sell", size = sell_size, order_type = "limit", price = sell_px }
    end
    if #orders <= 1 then
        -- 两手都没挂出(量太小/名义不足): **保持 need_rehang**(下一 tick 再试),
        -- 否则会永久不再挂单 —— 2026-09-23 发现的停摆 bug。
        if gate_blocked then
            -- 方向被门控: 必须主动撤掉旧挂单(否则旧卖单仍会被成交)
            return { { pair = pair, action = "cancel_pending" } }
        end
        skip_notional = skip_notional + 1
        return {}
    end
    need_rehang = false
    rehang_count = rehang_count + 1
    grid_buy_px = buy_px
    grid_sell_px = sell_px
    save_state(ctx)
    ctx:log(string.format(
        "[shannon_spot_grid] 网格重挂 #%d: 平衡价 %.4f -> 买 %.4f (%.6f) / 卖 %.4f (%.6f), 真实持仓 %.6f 账本规模 %.0f",
        rehang_count, balance_price, buy_px, buy_size, sell_px, sell_size, real, v_cap))
    return orders
end

function on_fill(ctx, fill)
    local px = fill.fill_price or balance_price
    local size = fill.fill_size or 0
    if pending_entry then
        -- 初始建仓成交: **该笔成交价即第一次平衡价格**(用户 2026-09-23 口径)
        pending_entry = false
        built = true
        balance_price = px
        entry_price = px
        entry_size = size
        if invested0 <= 0 then invested0 = ctx:equity() or 0 end
        v_reset(ctx, balance_price)
        ctx:log(string.format(
            "[shannon_spot_grid] 初始建仓成交 %.6f @ %.4f -> 平衡价 := %.4f(第一次平衡价格), 账本规模 %.0f",
            size, px, balance_price, v_cap))
    else
        -- 网格单成交: 平衡价 := 该笔成交价; **虚拟仓位按真实成交同步调整**(用户 2026-09-23 口径:
        -- 初始建仓时初始化一次, 之后只按真实成交加减, 余额不足时才重新初始化)。
        balance_price = px
        if fill.side == "buy" then
            v_coin = v_coin + size
            v_cash = v_cash - size * px
        else
            v_coin = v_coin - size
            v_cash = v_cash + size * px
        end
    end
    if fill.side == "buy" then
        buy_count = buy_count + 1
    else
        sell_count = sell_count + 1
    end
    last_side = fill.side
    fill_count = fill_count + 1
    need_rehang = true
    local pair_ = ctx:config_str("pair")
    local pos_now = ctx:position_size(pair_) or 0
    local eq_now = ctx:equity() or 0
    -- 实际仓位清空 → 结束策略(用户 2026-09-23)
    if pos_now <= 0 then
        finished = true
        ctx:log("[shannon_spot_grid] 实际仓位已清空 → 结束策略(不再交易)")
    end
    save_state(ctx)
    ctx:log(string.format(
        "[FILL] #%d ts=%d %s size=%.6f px=%.4f notional=%.2f | 权益=%.2f 持仓=%.6f 平衡价(新)=%.4f 账本(币=%.4f 现金=%.2f 规模=%.0f)",
        fill_count, last_bar_ts or 0, fill.side, size, px, size * px, eq_now, pos_now, balance_price, v_coin, v_cash, v_cap))
end

function on_stop(ctx)
    log_state(ctx, "收尾")
    if cfg_str(ctx, "regime_filter", "ema200") == "ema200" then
        ctx:log(string.format(
            "[shannon_spot_grid] 趋势判据统计: 末态 %s / 切换 %d 次 / tick 数 BULL %d RANGE %d " ..
            "BEAR %d / 拦买 %d 拦卖 %d / 判据未就绪跳过 %d",
            string.upper(regime_state), regime_switch, ticks_bull, ticks_range, ticks_bear,
            regime_block_buy, regime_block_sell, regime_ready_skip))
    end
    ctx:log(string.format(
        "[shannon_spot_grid] 停机(**不清仓**): 成交 %d(买 %d 卖 %d) / 跳过(ATR未就绪 %d, 小名义 %d, 判据拦 %d) / " ..
        "门控(BULL拦卖 %d BEAR拦买 %d) / 受限(买入现金截断 %d) / %s / 账本末态(币 %.6f 现金 %.0f 总计 %.0f) 杠杆 %.1f",
        fill_count, buy_count, sell_count, skip_no_atr, skip_notional, skip_signal,
        regime_block_sell, regime_block_buy, buy_capped, finished and "已结束(仓位清空)" or (halted and "已停机(R7 成本门槛)" or "正常运行"),
        v_coin, v_cash, v_coin * last_price + v_cash, num(ctx, "leverage_mult", 2)) .. string.format(" (账本规模 %.0f)", v_cap))
    -- 收益分解(用户 2026-09-23): 持仓收益 = 期末持仓量 ×(末价 − 初始建仓价); 交易收益 = 总收益 − 持仓收益。
    local q1 = ctx:position_size(ctx:config_str("pair")) or 0
    local eq1 = ctx:equity() or 0
    local p1 = last_price or 0
    local inv = invested0
    if inv <= 0 then inv = num(ctx, "real_cash", 0) end
    if p1 > 0 and inv > 0 then
        local total = eq1 - inv
        local hold = 0
        if entry_price > 0 then hold = q1 * (p1 - entry_price) end
        ctx:log(string.format(
            "[shannon_spot_grid] 收益分解: 投入 %.0f | 期初建仓 %.6f@%.4f | 期末持仓 %.6f@%.4f 权益 %.2f | " ..
            "合计收益 %+.2f = 持仓收益 %+.2f + 交易收益 %+.2f",
            inv, entry_size, entry_price, q1, p1, eq1, total, hold, total - hold))
    end
end
