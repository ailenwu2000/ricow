-- 香农 50:50 中轴再平衡策略 v4 (内置参考实现, 语义对齐 Rust 版)。
-- 设计:
--   - 建仓: 启动后首个 tick 市价买入 target_ratio 权益 (默认 0.5 = 50%, 一次单)。
--   - 再平衡: 每 tick 计算 ratio = 持仓市值/权益;
--       ratio > target_ratio+band → 卖 (价格上涨失衡, 锁利回平衡);
--       ratio < target_ratio-band → 买 (价格下跌失衡, 摊低成本回平衡);
--     恢复量为回平衡所需精确量 (|差额|/价), 非固定份。
--   - band (再平衡带宽) 默认随波动率自适应:
--       band_eff = max(rebalance_band, atr_mult × ATR(atr_period)/price)
--     高波动标的自动放宽 band (少交易防摩擦), 低波动由 rebalance_band 保底;
--     atr_period ≤ 0 或 ATR 数据不足 (nil) → 回退固定 rebalance_band。
--   - 极端行情暂停 (可选): 最近已收盘 bar 涨跌 ≥ pause_pct → 暂停 pause_bars 根。
-- 参数: pair / order_size / target_ratio (默认 0.5, 建议 0.2~0.8) /
--       rebalance_band (默认 0.005, band 下限, 价格当量须 > 2×双边手续费 0.2%) /
--       atr_period (默认 14, 0=禁用 ATR) / atr_mult (默认 1.0) /
--       pause_pct / pause_bars (默认 6) /
--       dd_stop_pct (默认 0 = 关闭): **策略自管回撤**阈值 —— 权益从峰值回撤达该比例即停止买入
--         (2026-09-15 起平台不再代做亏损熔断, 这类判断属于策略; 平台提供 ctx:equity()/ctx:net_pnl())。
-- 用法: ricow backtest --strategy shannon_grid --pair ETH
-- 复制到 strategies/scripts/ 即自定义。

pair = nil
order_size = 0
target_ratio = 0.5
rebalance_band = 0.005
atr_period = 14
atr_mult = 1.0
pause_pct = 0
pause_bars = 6
dd_stop_pct = 0
peak_equity = 0
dd_halted = false
dd_logged = false
quote_asset = nil
built = false
initialized = false
paused_until = 0
tick_count = 0

-- 报价资产推导在 exec.detect_quote (pair 尾部匹配, 与旧 detect_quote 一致)。

-- 权益 ≈ quote 余额 + 持仓市值。
function equity(ctx)
    return ctx:balance(quote_asset) + ctx:position_size(pair) * (ctx:price(pair) or 0)
end

-- 策略自管回撤 (2026-09-15, 默认关闭): 权益峰值回撤 ≥ dd_stop_pct → 停止买入。
-- 平台已不再代做亏损熔断 —— 这类判断由策略自己表达 (0.6.x 前是平台的"二级熔断")。
-- 两种口径都可自选: 权益回撤用 ctx:equity(), 已实现盈亏回撤用 ctx:net_pnl()。
function drawdown_halted(ctx)
    if dd_stop_pct <= 0 then
        return false
    end
    local eq = ctx:equity()
    if peak_equity <= 0 then
        peak_equity = eq
        return false
    end
    if eq > peak_equity then
        peak_equity = eq
    end
    return (peak_equity - eq) / peak_equity >= dd_stop_pct
end

-- 中轴再平衡决策 (纯函数): 目标 target_ratio, 失衡 < band → 不交易。
-- 返回 side, size; 无需交易返回 nil。
function rebalance(equity_v, pos_value, price, band, ratio)
    if price <= 0 or equity_v <= 0 then
        return nil
    end
    local diff = equity_v * ratio - pos_value
    if math.abs(diff) < equity_v * band then
        return nil
    end
    local size = math.abs(diff) / price
    if size <= 0 then
        return nil
    end
    if diff > 0 then
        return "buy", size
    end
    return "sell", size
end

function on_init(ctx)
    pair = ctx:config_str("pair")
    order_size = ctx:config_f64("order_size")
    local tr = ctx:config_f64("target_ratio")
    if tr > 0 and tr <= 1 then target_ratio = tr end  -- clamp 到 (0,1], 超界回默认
    local band = ctx:config_f64("rebalance_band")
    if band > 0 then rebalance_band = band end
    local ap = ctx:config_f64("atr_period")
    if ap >= 0 then atr_period = ap end              -- CLI 注入默认 14; 0 = 禁用 ATR
    local am = ctx:config_f64("atr_mult")
    if am >= 0 then atr_mult = am end                -- CLI 注入默认 1.0; 0 = 禁用 ATR
    pause_pct = ctx:config_f64("pause_pct")
    local bars = ctx:config_i64("pause_bars")
    if bars >= 1 then pause_bars = bars end
    dd_stop_pct = ctx:config_f64("dd_stop_pct")   -- 0 = 关闭 (默认); >0 = 策略自管回撤阈值
    quote_asset = exec.detect_quote(pair)
    built = false
    initialized = false
    paused_until = 0
    tick_count = 0
    peak_equity = 0
    dd_halted = false
    dd_logged = false
    ctx:log("shannon_grid: 初始化 " .. pair .. " order_size=" .. order_size
        .. " target_ratio=" .. target_ratio .. " band=" .. rebalance_band
        .. " atr_period=" .. atr_period .. " atr_mult=" .. atr_mult)
end

-- 有效带宽: ATR 自适应 (默认); atr_mult=0 或 ATR 数据不足 → 固定 rebalance_band。
function effective_band(ctx, price)
    if atr_mult <= 0 then
        return rebalance_band
    end
    local atr = ctx:atr(pair, atr_period)
    if not atr or price <= 0 then
        return rebalance_band
    end
    local atr_band = atr_mult * atr / price
    if atr_band > rebalance_band then
        return atr_band
    end
    return rebalance_band
end

-- 建仓: 一次市价买入 target_ratio 权益。
function build_position(ctx, price)
    local size = ctx:balance(quote_asset) * target_ratio / price
    built = true
    if size <= 0 then
        ctx:log("shannon_grid: 建仓跳过 (无可用资金或价格无效)")
        return {}
    end
    ctx:log("shannon_grid: 建仓买入 " .. math.floor(target_ratio * 100 + 0.5)
        .. "% 权益 @ ~" .. price .. " (size=" .. size .. ")")
    return { { pair = pair, side = "buy", size = size, order_type = "market" } }
end

-- 中轴再平衡: 失衡超 band → 一次交易精确恢复 target_ratio。
function tick_rebalance(ctx, price)
    local pos_size = ctx:position_size(pair)
    local pos_value = pos_size * price
    local band = effective_band(ctx, price)
    local side, size = rebalance(equity(ctx), pos_value, price, band, target_ratio)
    if not side then
        return {}
    end
    -- 守卫: 卖需有持仓; 买不超过可用现金。
    if side == "sell" and pos_size <= 0 then
        return {}
    end
    if side == "buy" then
        if dd_halted then
            return {}
        end
        local max_buy = ctx:balance(quote_asset) / price
        if size > max_buy then size = max_buy end
    end
    if size <= 0 then
        return {}
    end
    ctx:log("shannon_grid: 再平衡 " .. side .. " " .. size .. " (回 "
        .. math.floor(target_ratio * 100 + 0.5) .. "%, band=" .. band .. ")")
    return { { pair = pair, side = side, size = size, order_type = "market" } }
end

function on_tick(ctx)
    tick_count = tick_count + 1
    if not pair or pair == "" then
        return {}
    end
    local price = ctx:price(pair)
    if not price then
        return {}
    end

    -- 重启语义: 已有持仓 → 跳过建仓直接再平衡。
    if not initialized then
        initialized = true
        built = ctx:position_size(pair) > 0
        if built then
            ctx:log("shannon_grid: 检测到持仓, 跳过建仓直接再平衡")
        end
    end

    -- 极端行情暂停 (防滑点, 非止损): 单 bar 涨跌 ≥ pause_pct → 暂停 pause_bars 根。
    if pause_pct > 0 then
        local ks = ctx:klines(pair)
        if ks and #ks > 0 then
            local k = ks[#ks]
            if k.open > 0 then
                local range = math.abs(k.close - k.open) / k.open
                if range >= pause_pct then
                    paused_until = tick_count + pause_bars
                    ctx:log("shannon_grid: 极端行情暂停 " .. pause_bars .. " bar (涨跌 "
                        .. math.floor(range * 10000 + 0.5) / 100 .. "%)")
                end
            end
        end
        if tick_count < paused_until then
            return {}
        end
    end

    dd_halted = drawdown_halted(ctx)
    if dd_halted and not dd_logged then
        dd_logged = true
        ctx:log("shannon_grid: 自管回撤触发, 停止买入 (权益=" .. ctx:equity()
            .. ", 峰值=" .. peak_equity .. ", 阈值=" .. dd_stop_pct .. ")")
    end

    if not built then
        return build_position(ctx, price)
    end
    return tick_rebalance(ctx, price)
end
