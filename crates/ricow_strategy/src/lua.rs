//! Lua 自定义策略运行时。
//!
//! 提供:
//! - `validate_lua`: 编译校验 (门禁, 不执行)
//! - `LuaStrategy`: 实现 `Strategy` trait, 回调转发到 Lua 脚本函数
//!
//! 安全边界 (见 lua_sandbox.rs): 禁 require/loadstring/loadfile/dofile + 指令预算 + print 重定向。
//! 脚本只通过返回订单表数组影响策略行为; ctx 为只读快照 (LuaCtxData userdata)。

use std::collections::HashMap;

use chrono::{Datelike, Timelike};
use ricow_core::{Kline, OrderFill, OrderRequest, OrderSide, OrderType, OrderUpdate};
use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::config::{ConfigValue, StrategyConfig};
use crate::context::Context;
use crate::indicators_api;
use crate::lua_sandbox::create_lua_sandbox;
use crate::strategy::Strategy;

// ============================================================================
// 编译门禁
// ============================================================================

/// 编译校验 (门禁)。只编译不执行: 顶层代码不得在门禁阶段运行 (防副作用/内存炸弹)。
/// 校验 `script` 参数是不是**Lua 代码**(而非文件名)。
///
/// 实测坑(2026-09-14): 把 `script` 写成 `"grid.lua"` 时, 引擎会把它**当代码**编译 →
/// 报 `syntax error near '-'` 之类难以理解的错误。要引用脚本文件应使用 `script_path`(相对 `strategies/`)。
pub fn validate_script_source(code: &str) -> Result<(), String> {
    let t = code.trim();
    if !t.contains('\n') && t.len() <= 120 && t.to_ascii_lowercase().ends_with(".lua") {
        return Err(format!(
            "`script` 参数是**内联 Lua 代码字符串**, 但收到的是文件名 '{t}'。\n\
             要引用脚本文件请改用它: script_path = \"scripts/{t}\" (相对 strategies/ 目录);\n\
             或直接把 Lua 代码写进 script(部署产生的 TOML 就是内联形式)。"
        ));
    }
    Ok(())
}

pub fn validate_lua(code: &str) -> Result<(), String> {
    let (lua, _budget) = create_lua_sandbox();
    lua.load(code).into_function().map(|_| ()).map_err(|e| format!("脚本编译错误:\n{e}"))
}

// ============================================================================
// LuaCtxData — 只读快照容器 (userdata)
// ============================================================================

/// Lua 脚本可读取的 Context 快照。每个回调周期由 `fill_snapshot` 填充, 以 userdata 传进脚本。
/// 方法 `new`/`best_bid`/`best_ask` 为 `pub(crate)` — exec.rs 的 exec.side_order 跨模块取价。
#[derive(Clone)]
pub(crate) struct LuaCtxData {
    prices: HashMap<String, f64>,
    best_bids: HashMap<String, f64>,
    best_asks: HashMap<String, f64>,
    position_sides: HashMap<String, String>,
    position_sizes: HashMap<String, f64>,
    position_entries: HashMap<String, f64>,
    /// 方向仓快照 (D9/hedge): key = "{pair}|long|" / "{pair}|short", 值 = (size, entry_price)。
    directionals: HashMap<String, (f64, f64)>,
    balances: HashMap<String, f64>,
    /// 已实现净盈亏 (报价币计, 已扣手续费): `ctx:net_pnl()`。020 新增 ——
    /// 平台不再代做亏损熔断, 回撤/止损规则由策略自己实现, 这是它的输入。
    net_pnl: f64,
    /// 总权益: 现货 = 报价现金 + 持仓市值; 合约 = 钱包现金 + 未实现盈亏。`ctx:equity()`。
    equity: f64,
    config_f64_map: HashMap<String, f64>,
    config_i64_map: HashMap<String, i64>,
    config_str_map: HashMap<String, String>,
    config_bool_map: HashMap<String, bool>,
    /// 当前 tick 时间 (UTC); None = 通道不可用 (单标的/实盘未接)。ctx:now() 用。
    now: Option<chrono::DateTime<chrono::Utc>>,
    klines_map: HashMap<String, Vec<Kline>>,
    /// 组合信号模式标志 (bs_momentum Lua 化, T3): true → ctx:klines 返回全段
    /// (引擎已截断至执行日, ≥253 根供 IBD RS/EMA200 打分), 不套单标的 100 根 cap;
    /// false (默认) → 单标的路径维持 cap 100 (行为边界, 回归约束)。
    full_klines: bool,
}

impl LuaCtxData {
    pub(crate) fn new() -> Self {
        Self {
            prices: HashMap::new(),
            best_bids: HashMap::new(),
            best_asks: HashMap::new(),
            position_sides: HashMap::new(),
            position_sizes: HashMap::new(),
            position_entries: HashMap::new(),
            directionals: HashMap::new(),
            balances: HashMap::new(),
            net_pnl: 0.0,
            equity: 0.0,
            config_f64_map: HashMap::new(),
            config_i64_map: HashMap::new(),
            config_str_map: HashMap::new(),
            config_bool_map: HashMap::new(),
            now: None,
            klines_map: HashMap::new(),
            full_klines: false,
        }
    }

    fn price(&self, pair: &str) -> Option<f64> {
        self.prices.get(pair).copied()
    }
    pub(crate) fn best_bid(&self, pair: &str) -> Option<f64> {
        self.best_bids.get(pair).copied()
    }
    pub(crate) fn best_ask(&self, pair: &str) -> Option<f64> {
        self.best_asks.get(pair).copied()
    }
    fn position_side(&self, pair: &str) -> String {
        self.position_sides.get(pair).cloned().unwrap_or_else(|| "none".into())
    }
    fn position_size(&self, pair: &str) -> f64 {
        self.position_sizes.get(pair).copied().unwrap_or(0.0)
    }
    fn position_entry(&self, pair: &str) -> f64 {
        self.position_entries.get(pair).copied().unwrap_or(0.0)
    }
    /// 方向仓数量: side 为 "long"/"short"; 无仓或非法 side → 0。
    fn directional_size(&self, pair: &str, side: &str) -> f64 {
        self.directionals.get(&format!("{pair}|{side}")).map(|(s, _)| *s).unwrap_or(0.0)
    }
    /// 方向仓开仓均价: side 为 "long"/"short"; 无仓或非法 side → 0。
    fn directional_entry(&self, pair: &str, side: &str) -> f64 {
        self.directionals.get(&format!("{pair}|{side}")).map(|(_, e)| *e).unwrap_or(0.0)
    }
    fn balance(&self, asset: &str) -> f64 {
        self.balances.get(asset).copied().unwrap_or(0.0)
    }
    fn config_f64(&self, key: &str) -> f64 {
        self.config_f64_map.get(key).copied().unwrap_or(0.0)
    }
    fn config_i64(&self, key: &str) -> i64 {
        self.config_i64_map.get(key).copied().unwrap_or(0)
    }
    fn config_str(&self, key: &str) -> String {
        self.config_str_map.get(key).cloned().unwrap_or_default()
    }
    fn config_bool(&self, key: &str) -> bool {
        self.config_bool_map.get(key).copied().unwrap_or(false)
    }
    /// 已收盘 K 线历史 (指标数据源, 无前视)。
    fn klines(&self, pair: &str) -> Option<Vec<Kline>> {
        self.klines_map.get(pair).cloned()
    }
}

impl UserData for LuaCtxData {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("price", |_, data, pair: String| Ok(data.price(&pair)));
        methods.add_method("best_bid", |_, data, pair: String| Ok(data.best_bid(&pair)));
        methods.add_method("best_ask", |_, data, pair: String| Ok(data.best_ask(&pair)));
        methods.add_method("position_side", |_, data, pair: String| Ok(data.position_side(&pair)));
        methods.add_method("position_size", |_, data, pair: String| Ok(data.position_size(&pair)));
        methods
            .add_method("position_entry", |_, data, pair: String| Ok(data.position_entry(&pair)));
        // 方向仓查询 (D9/hedge): ctx:pos_size(pair, "long") / ctx:pos_entry(pair, "short")。
        // 现货恒 long (size = 净仓), short 恒 0。one-way 下 short/long 单独可见。
        methods.add_method("pos_size", |_, data, (pair, side): (String, String)| {
            Ok(data.directional_size(&pair, &side))
        });
        methods.add_method("pos_entry", |_, data, (pair, side): (String, String)| {
            Ok(data.directional_entry(&pair, &side))
        });
        methods.add_method("balance", |_, data, asset: String| Ok(data.balance(&asset)));
        // 盈亏状态 (020): 平台不再代做亏损熔断/峰值回撤 —— 策略用这两个只读值自管风控。
        // net_pnl = 已实现净盈亏(报价币, 扣手续费); equity = 总权益(现货: 现金+持仓市值; 合约: 现金+未实现盈亏)。
        methods.add_method("net_pnl", |_, data, ()| Ok(data.net_pnl));
        methods.add_method("equity", |_, data, ()| Ok(data.equity));
        // ctx:now() — 当前 tick 时间 (UTC): {hour, minute, weekday(1=周一), ymd, ts}。
        // 回测 = 本 tick 已开盘 bar 的 open_time (无前视); 通道不可用 → nil (策略须 if t then 判断)。
        // 用途: 盘中策略只在"每日固定时刻"下单 (如美股开盘 1 小时后 = 14:30 UTC / 15:30 UTC 冬令),
        // 其余 tick 只作估值 (不吃仓位变动), 使组合回测能表达"盘中择时"。2026-09-11 新增。
        methods.add_method("now", |lua, data, ()| {
            let Some(t) = data.now else {
                return Ok(mlua::Value::Nil);
            };
            let tbl = lua.create_table()?;
            tbl.set("hour", t.hour())?;
            tbl.set("minute", t.minute())?;
            tbl.set("weekday", t.weekday().number_from_monday())?;
            tbl.set("ymd", t.format("%Y%m%d").to_string())?;
            tbl.set("ts", t.timestamp())?;
            Ok(mlua::Value::Table(tbl))
        });
        methods.add_method("config_f64", |_, data, key: String| Ok(data.config_f64(&key)));
        methods.add_method("config_i64", |_, data, key: String| Ok(data.config_i64(&key)));
        methods.add_method("config_str", |_, data, key: String| Ok(data.config_str(&key)));
        methods.add_method("config_bool", |_, data, key: String| Ok(data.config_bool(&key)));
        methods.add_method("log", |_, _data, msg: String| {
            tracing::info!(target: "lua_strategy", "{msg}");
            Ok(())
        });
        // 已收盘 K 线历史 (无前视): 返回 {open=, close=} 表数组。单标的路径最多最近
        // 100 根 (行为边界, 回归约束); 组合信号模式 (full_klines) 返回全段 — 引擎已按
        // 执行日截断, ≥253 根供长窗打分 (bs_momentum Lua 化 T3)。
        methods.add_method("klines", |lua, data, pair: String| {
            let k = data.klines(&pair).unwrap_or_default();
            let start = if data.full_klines { 0 } else { k.len().saturating_sub(100) };
            let t = lua.create_table()?;
            for (i, kline) in k.iter().skip(start).enumerate() {
                let row = lua.create_table()?;
                row.set("ts", kline.open_time.timestamp())?;
                row.set("open", kline.open.to_f64().unwrap_or(0.0))?;
                row.set("close", kline.close.to_f64().unwrap_or(0.0))?;
                row.set("volume", kline.volume.to_f64().unwrap_or(0.0))?;
                t.set(i + 1, row)?;
            }
            Ok(t)
        });
        // 指标 API (基于已收盘序列, 数据不足返回 nil)。
        methods.add_method("ema", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::ema(&k, period))
        });
        methods.add_method("sma", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::sma(&k, period))
        });
        methods.add_method("wma", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::wma(&k, period))
        });
        methods.add_method("rsi", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::rsi(&k, period))
        });
        methods.add_method("macd", |_, data, pair: String| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::macd(&k))
        });
        methods.add_method("boll", |_, data, (pair, period, dev): (String, usize, f64)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::boll(&k, period, dev))
        });
        methods.add_method("atr", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::atr(&k, period))
        });
        methods.add_method("adx", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::adx(&k, period))
        });
        methods.add_method("stoch", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::stoch(&k, period))
        });
        methods.add_method("cci", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::cci(&k, period))
        });
        methods.add_method("roc", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::roc(&k, period))
        });
        methods.add_method("mom", |_, data, (pair, period): (String, usize)| {
            let k = data.klines(&pair).unwrap_or_default();
            Ok(indicators_api::mom(&k, period))
        });
    }
}

// ============================================================================
// LuaStrategy
// ============================================================================

/// Lua 策略: 沙箱引擎实例 + 全局表状态跨回调保持 (模块级 let 状态跨 tick 持续)。
pub struct LuaStrategy {
    config: StrategyConfig,
    lua: Lua,
    budget: crate::lua_sandbox::SandboxBudget,
    instance_id: String,
}

impl std::fmt::Debug for LuaStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaStrategy")
            .field("name", &self.config.name)
            .field("instance_id", &self.instance_id)
            .finish()
    }
}

impl LuaStrategy {
    /// 从源码构建策略: 沙箱引擎 + 编译 + 执行模块级语句 (函数定义/模块级状态)。
    /// 统一注册 exec 执行组件库 (Rust 实现): 所有 Lua 策略 (CLI/MCP/TOML/回测) 均可用 exec.*;
    /// 用户脚本后执行可覆盖库函数 (复制即自定义)。
    pub fn from_source(code: &str, config: StrategyConfig) -> Result<Self, String> {
        let (lua, budget) = create_lua_sandbox();
        // 注册 exec 执行组件 (Rust 实现, 全局表): 所有 Lua 策略 (CLI/MCP/TOML/回测) 均可用
        // exec.*; 用户脚本后执行可覆盖库函数 (复制即自定义)。
        crate::exec::register(&lua).map_err(|e| format!("exec 组件注册失败: {e}"))?;
        lua.load(code).exec().map_err(|e| format!("脚本错误 (编译或顶层执行):\n{e}"))?;
        let instance_id = uuid::Uuid::new_v4().to_string();
        Ok(Self { config, lua, budget, instance_id })
    }

    fn fill_snapshot(&self, ctx: &dyn Context, data: &mut LuaCtxData) {
        // 组合信号模式判据 (纯 config 判断, 零 trait 扩展, Y3): config.params 有
        // universe 键 (逗号串, bs_momentum 装配层注入) → 遍历池逐只填价格/持仓/信号线
        // 快照 + full_klines=true (klines 全段 ≥253, 引擎已按执行日截断无前视);
        // 无 universe → 现单标的路径 (只填 config.pair, klines cap 100 回归约束)。
        let universe: Vec<String> = ctx
            .config()
            .get_str("universe")
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        data.full_klines = !universe.is_empty();
        data.now = ctx.now_utc();
        let pairs: Vec<String> = if universe.is_empty() {
            vec![self.config.get_str("pair").unwrap_or("ETH").to_string()]
        } else {
            universe
        };

        // 未实现盈亏合计 (合约权益口径用; 现货不用)。
        let mut unrealized = 0.0f64;

        for pair in pairs {
            if let Some(price) = ctx.price(&pair) {
                if let Some(f) = price.to_f64() {
                    data.prices.insert(pair.clone(), f);
                }
            }
            if let Some(ob) = ctx.orderbook(&pair) {
                if let Some(bid) = ob.best_bid() {
                    if let Some(f) = bid.price.to_f64() {
                        data.best_bids.insert(pair.clone(), f);
                    }
                }
                if let Some(ask) = ob.best_ask() {
                    if let Some(f) = ask.price.to_f64() {
                        data.best_asks.insert(pair.clone(), f);
                    }
                }
            }
            if let Some(pos) = ctx.position(&pair) {
                let side = if pos.size <= Decimal::ZERO {
                    "none"
                } else if pos.side == OrderSide::Buy {
                    "long"
                } else {
                    "short"
                };
                data.position_sides.insert(pair.clone(), side.to_string());
                data.position_sizes.insert(pair.clone(), pos.size.to_f64().unwrap_or(0.0));
                data.position_entries
                    .insert(pair.clone(), pos.entry_price.to_f64().unwrap_or(0.0));
                unrealized += pos.unrealized_pnl.to_f64().unwrap_or(0.0);
            }
            // 方向仓快照 (D9/hedge): 现货只填 long (= 净仓); 合约按 (pair, side) 各自方向仓。
            for (side, label) in [(OrderSide::Buy, "long"), (OrderSide::Sell, "short")] {
                if let Some(p) = ctx.position_directional(&pair, side) {
                    let key = format!("{pair}|{label}");
                    data.directionals
                        .insert(key, (p.size.to_f64().unwrap_or(0.0), p.entry_price.to_f64().unwrap_or(0.0)));
                }
            }
            if let Some(k) = ctx.klines(&pair) {
                data.klines_map.insert(pair.clone(), k);
            }
        }
        // 同步常见报价资产余额 (Lua 策略经 balance() 读取; USDT 为 CLI 默认, USDC 为历史测试口径)。
        for asset in ["USDT", "USDC", "BUSD", "FDUSD", "TUSD", "DAI"] {
            if let Some(bal) = ctx.balance(asset) {
                if let Some(f) = bal.to_f64() {
                    data.balances.insert(asset.to_string(), f);
                }
            }
        }
        // 盈亏状态快照 (020): 策略自管风控的输入 —— 已实现净盈亏与总权益。
        data.net_pnl = ctx.pnl().net_pnl().to_f64().unwrap_or(0.0);
        let cash: f64 = data.balances.values().sum();
        data.equity = if ctx.config().market == "futures" {
            // 合约: 持仓名义价值 ≠ 权益, 权益 = 钱包现金 + 未实现盈亏。
            cash + unrealized
        } else {
            // 现货: 现金 + 持仓市值 (数量 × 现价)。
            let holdings: f64 = data
                .position_sizes
                .iter()
                .filter_map(|(pair, size)| data.prices.get(pair).map(|p| size * p))
                .sum();
            cash + holdings
        };
        // config_*_map 从 ctx.config().params 构建 (而非 self.config): BacktestContext::new
        // 会把 resolve 后的最终撮合参数 (fee/slippage/leverage/mmr/funding) 回写进 ctx 的
        // config.params (Y7, bs_momentum Lua 化) — Lua 策略经 ctx:config_f64 读到的费率/
        // 滑点必须与撮合层 FeeModel 同源一致, 预算净回笼口径才能闭合。DryRun/Live 下
        // ctx.config 与 self.config 同值 (同一 StrategyConfig 克隆), 行为不变。
        for (key, value) in &ctx.config().params {
            match value {
                ConfigValue::Float(f) => {
                    data.config_f64_map.insert(key.clone(), *f);
                }
                ConfigValue::Integer(i) => {
                    data.config_i64_map.insert(key.clone(), *i);
                    data.config_f64_map.insert(key.clone(), *i as f64);
                }
                ConfigValue::Boolean(b) => {
                    data.config_bool_map.insert(key.clone(), *b);
                }
                ConfigValue::String(s) => {
                    data.config_str_map.insert(key.clone(), s.clone());
                    // CLI 直跑把网格类价格参数按 Decimal 字符串注入 (防精度噪声, run/backtest 的
                    // lower_price/upper_price); Lua 无 config_dec, 字符串数字同时进 config_f64_map,
                    // 否则 config_f64("lower_price") 恒 0 → ladder 直跑 0 成交 (Lua 化迁移遗留)。
                    if let Ok(f) = s.parse::<f64>() {
                        data.config_f64_map.insert(key.clone(), f);
                    }
                }
            }
        }
    }

    /// 调用脚本函数; 函数未定义返回 None (静默跳过), 其他运行时错误记日志后返回 None。
    fn call(&self, fn_name: &str, data: LuaCtxData, extra: Vec<Value>) -> Option<Value> {
        // 指令预算按回调周期 (tick) 重置: 每个回调独立 1M 条预算, 不跨回调累积。
        self.budget.reset();
        let globals = self.lua.globals();
        let func: Function = match globals.get(fn_name) {
            Ok(f) => f,
            Err(_) => return None,
        };
        let userdata = match self.lua.create_userdata(data) {
            Ok(u) => u,
            Err(e) => {
                tracing::error!(target: "lua_strategy", instance = %self.instance_id, "{fn_name}: 快照构造失败: {e}");
                return None;
            }
        };
        let mut args = vec![mlua::Value::UserData(userdata)];
        args.extend(extra);
        let args_tuple: mlua::MultiValue = args.into_iter().collect();
        match func.call::<Value>(args_tuple) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!(target: "lua_strategy", instance = %self.instance_id, "{fn_name}: {e}");
                None
            }
        }
    }

    /// 将 Lua 订单表数组解析为 Vec<OrderRequest>。
    fn parse_orders(&self, table: &Table) -> Vec<OrderRequest> {
        let mut orders = Vec::new();
        for item in table.sequence_values::<Table>() {
            let Ok(t) = item else { continue };
            let pair = t.get::<String>("pair").unwrap_or_default();
            let side = match t.get::<String>("side").as_deref() {
                Ok(s) if s.eq_ignore_ascii_case("sell") => OrderSide::Sell,
                Ok(_) => OrderSide::Buy,
                Err(_) => {
                    tracing::warn!(target: "lua_strategy", instance = %self.instance_id, "订单缺 side 字段, 默认 buy");
                    OrderSide::Buy
                }
            };
            let size = t
                .get::<f64>("size")
                .ok()
                .and_then(Decimal::from_f64_retain)
                .unwrap_or(Decimal::ZERO);
            if size <= Decimal::ZERO {
                tracing::warn!(target: "lua_strategy", instance = %self.instance_id, "订单 size 缺失/非法, 丢弃: pair={pair}");
            }
            let price =
                t.get::<Option<f64>>("price").ok().flatten().and_then(Decimal::from_f64_retain);
            let order_type = match t.get::<String>("order_type").as_deref() {
                Ok(s) if s.eq_ignore_ascii_case("market") => OrderType::Market,
                Ok(s) if s.eq_ignore_ascii_case("limit") => OrderType::Limit,
                Ok(s) => {
                    tracing::warn!(target: "lua_strategy", instance = %self.instance_id, "未知 order_type '{s}', 默认 limit");
                    OrderType::Limit
                }
                Err(_) => OrderType::Limit,
            };
            let reduce_only = t.get::<Option<bool>>("reduce_only").ok().flatten().unwrap_or(false);
            // 可选 position_side (合约 hedge): "long"/"short"; 非法值告警并忽略 (按 one-way 语义)。
            let position_side = match t.get::<Option<String>>("position_side").ok().flatten() {
                Some(s) if s.eq_ignore_ascii_case("long") => Some("long".to_string()),
                Some(s) if s.eq_ignore_ascii_case("short") => Some("short".to_string()),
                Some(_) => {
                    tracing::warn!(target: "lua_strategy", "忽略非法 position_side, 按 one-way 处理");
                    None
                }
                None => None,
            };

            if size <= Decimal::ZERO || pair.is_empty() {
                continue;
            }
            orders.push(OrderRequest {
                client_order_id: String::new(),
                pair,
                side,
                order_type,
                price,
                size,
                reduce_only,
                position_side,
            });
        }
        orders
    }

    /// 将成交事件转成 Lua 表 (on_fill 回调参数)。
    fn fill_to_table(&self, fill: &OrderFill) -> Result<Table, mlua::Error> {
        let t = self.lua.create_table()?;
        t.set("pair", fill.pair.clone())?;
        t.set("fill_price", fill.fill_price.to_f64().unwrap_or(0.0))?;
        t.set("fill_size", fill.fill_size.to_f64().unwrap_or(0.0))?;
        let side = match fill.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        t.set("side", side)?;
        t.set("fee", fill.fee.to_f64().unwrap_or(0.0))?;
        Ok(t)
    }
}

impl Strategy for LuaStrategy {
    fn on_init(&mut self, ctx: &mut dyn Context) {
        let mut data = LuaCtxData::new();
        self.fill_snapshot(ctx, &mut data);
        self.call("on_init", data, vec![]);
    }

    fn on_tick(&mut self, ctx: &mut dyn Context) -> Vec<OrderRequest> {
        let mut data = LuaCtxData::new();
        self.fill_snapshot(ctx, &mut data);

        let result = match self.call("on_tick", data, vec![]) {
            Some(v) => v,
            None => return vec![],
        };
        match result {
            Value::Table(t) => self.parse_orders(&t),
            _ => vec![],
        }
    }

    fn on_fill(&mut self, ctx: &mut dyn Context, fill: OrderFill) {
        let mut data = LuaCtxData::new();
        self.fill_snapshot(ctx, &mut data);
        if let Ok(t) = self.fill_to_table(&fill) {
            self.call("on_fill", data, vec![Value::Table(t)]);
        }
    }

    fn on_order_update(&mut self, _ctx: &mut dyn Context, _update: OrderUpdate) {
        // Lua 策略不处理 order_update 回调。
    }

    fn on_stop(&mut self, ctx: &mut dyn Context) {
        let mut data = LuaCtxData::new();
        self.fill_snapshot(ctx, &mut data);
        self.call("on_stop", data, vec![]);
    }

    /// 脚本里定义了 `on_stop` 才算实现了清理 (未定义 → 引擎提示手工处理)。
    fn has_on_stop(&self) -> bool {
        let globals = self.lua.globals();
        matches!(globals.get::<Function>("on_stop"), Ok(_))
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::BacktestContext;
    use ricow_core::{Balance, Kline};
    use rust_decimal_macros::dec;

    fn sample_kline() -> Kline {
        Kline {
            open_time: chrono::Utc::now(),
            open: dec!(3000),
            high: dec!(3010),
            low: dec!(2990),
            close: dec!(3005),
            volume: dec!(100),
            close_time: chrono::Utc::now(),
        }
    }

    fn test_config(script: &str) -> StrategyConfig {
        let mut params = HashMap::new();
        params.insert("script".into(), ConfigValue::String(script.to_string()));
        params.insert("pair".into(), ConfigValue::String("ETH".into()));
        StrategyConfig {
            name: "t".into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            risk: None,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        }
    }

    /// 集成: LuaStrategy::on_tick 在 BacktestContext 下应返回订单。
    #[test]
    fn test_on_tick_returns_orders() {
        let script = r#"
            function on_tick(ctx)
                local p = ctx:price("ETH")
                if p and p > 2990 then
                    return { { pair = "ETH", side = "buy", size = 1, order_type = "market" } }
                end
                return {}
            end
        "#;
        let config = test_config(script);
        let strategy = LuaStrategy::from_source(script, config).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut strategy = strategy;
        ctx.step_bar(sample_kline());
        let orders = strategy.on_tick(&mut ctx);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].pair, "ETH");
        assert_eq!(orders[0].side, OrderSide::Buy);
        assert_eq!(orders[0].size, dec!(1));
        assert_eq!(orders[0].order_type, OrderType::Market);
    }

    /// String 数字参数 (CLI 直跑按 Decimal 字符串注入, 防精度噪声) 对 config_f64 可读 —
    /// Lua 化迁移遗留修复: 否则 ladder 直跑 lower_price/upper_price 恒 0 → 0 成交。
    #[test]
    fn test_config_f64_reads_string_number() {
        let script = r#"
            function on_tick(ctx)
                local lower = ctx:config_f64("lower_price")
                if lower > 0 then
                    return { { pair = "ETH", side = "buy", size = 1, price = lower, order_type = "limit" } }
                end
                return {}
            end
        "#;
        let mut params = HashMap::new();
        params.insert("script".into(), ConfigValue::String(script.to_string()));
        params.insert("pair".into(), ConfigValue::String("ETH".into()));
        params.insert("lower_price".into(), ConfigValue::String("2440.03".into()));
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            risk: None,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut strategy = LuaStrategy::from_source(script, config.clone()).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        ctx.step_bar(sample_kline());
        let orders = strategy.on_tick(&mut ctx);
        assert_eq!(orders.len(), 1, "String 数字应能被 config_f64 读到");
        let price = orders[0].price.unwrap().to_f64().unwrap();
        assert!((price - 2440.03).abs() < 0.001, "lower_price = {}, got {price}", price);
    }

    #[test]
    fn test_syntax_error_rejected() {
        let err = LuaStrategy::from_source("function on_tick(ctx) 语法错误!!!", test_config(""))
            .expect_err("语法错误应被拒绝");
        assert!(err.contains("脚本错误"));
    }

    #[test]
    fn test_validate_lua_gate() {
        assert!(validate_lua("function on_tick(ctx) return {} end").is_ok());
        assert!(validate_lua("this is not lua @@@").is_err());
    }

    #[test]
    fn test_validate_lua_no_toplevel_exec() {
        // 回归: 门禁只编译不执行 — 顶层有运行时错误 (语法合法) 的代码,
        // validate_lua 应通过 (编译期合法), 由 from_source (实际装载) 拒绝。
        let code = "local x = {} local y = x + 1 function on_tick(ctx) return {} end";
        assert!(validate_lua(code).is_ok(), "门禁不应执行顶层代码");
        let err = LuaStrategy::from_source(code, test_config("")).expect_err("装载时应拒绝");
        assert!(err.contains("脚本错误") || !err.is_empty());
    }

    #[test]
    fn test_budget_resets_per_tick() {
        // 回归: 指令预算按 tick 重置。实测 for i=1,N do end 每迭代约 1 条指令,
        // 600,000 次迭代 ≈ 600,005 条/tick: 单 tick 未超 1M 预算,
        // 但两个 tick 累积 ≈ 1.2M > 1M → 修复前 (跨 tick 累积) 第二个 tick 在循环中途
        // 被预算中断 → 订单丢失返回空 → 本测试失败。
        // 注意: 中断时 call() 记日志返回 None (不 panic), 必须断言"返回订单"而非"返回空",
        // 否则无法区分预算中断与正常完成。
        let script = r#"
            function on_tick(ctx)
                for i = 1, 600000 do end
                return { { pair = "ETH", side = "buy", size = 1, order_type = "market" } }
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source(script, config).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // 两个 tick 都必须返回订单 (修复前第二个 tick 会被预算中断丢单)。
        for _ in 0..2 {
            ctx.step_bar(sample_kline());
            let orders = strategy.on_tick(&mut ctx);
            assert_eq!(orders.len(), 1, "预算按 tick 重置, 每个 tick 都应正常返回订单");
        }
    }

    /// 指标 API + 已收盘序列: EMA 交叉脚本在 BacktestContext 下回测出订单。
    #[test]
    fn test_ema_cross_in_backtest() {
        let script = r#"
            fast_prev = nil
            function on_tick(ctx)
                local fast = ctx:ema("ETH", 3)
                local slow = ctx:ema("ETH", 5)
                if fast and slow then
                    if fast > slow then
                        return { { pair = "ETH", side = "buy", size = 0.1, order_type = "market" } }
                    end
                end
                return {}
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source(script, config).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // 6 根上升 K 线: ema(3) > ema(5), 触发买单。
        for i in 0..6u32 {
            let p = rust_decimal::Decimal::from(3000 + i * 10);
            let k = Kline {
                open_time: chrono::Utc::now(),
                open: p,
                high: p + dec!(5),
                low: p - dec!(5),
                close: p + dec!(2),
                volume: dec!(1),
                close_time: chrono::Utc::now(),
            };
            ctx.step_bar(k);
            let orders = strategy.on_tick(&mut ctx);
            for req in orders {
                let _ = ctx.place_order(req);
            }
            ctx.drain_fills();
        }
        assert!(ctx.report().total_trades >= 1, "ema 交叉应触发成交");
    }

    /// 死循环脚本被指令预算拦截, 不挂死。
    #[test]
    fn test_infinite_loop_intercepted() {
        let script = "function on_tick(ctx) while true do end end";
        let config = test_config(script);
        let strategy = LuaStrategy::from_source(script, config).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut strategy = strategy;
        ctx.step_bar(sample_kline());
        // 死循环被预算拦截 → 记日志返回空订单, 不 panic 不挂死。
        let orders = strategy.on_tick(&mut ctx);
        assert!(orders.is_empty());
    }

    /// 模块级状态跨 tick 保持。
    #[test]
    fn test_state_persists_across_ticks() {
        let script = r#"
            tick_count = 0
            function on_tick(ctx)
                tick_count = tick_count + 1
                if tick_count >= 3 then
                    return { { pair = "ETH", side = "buy", size = 0.1, order_type = "market" } }
                end
                return {}
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source(script, config).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut total = 0;
        for _ in 0..4 {
            ctx.step_bar(sample_kline());
            total += strategy.on_tick(&mut ctx).len();
        }
        // 第 3、4 tick 各返回 1 单。
        assert_eq!(total, 2);
    }

    /// T7: 合约 hedge 下方向仓查询 + position_side 字段透传 (D8/D9)。
    #[test]
    fn test_hedge_directional_query_and_position_side() {
        let script = r#"
            function on_tick(ctx)
                if ctx:pos_size("ETH", "long") == 0 then
                    return { { pair = "ETH", side = "buy", size = 2, order_type = "market",
                               position_side = "long" } }
                elseif ctx:pos_size("ETH", "short") == 0 then
                    return { { pair = "ETH", side = "sell", size = 1, order_type = "market",
                               position_side = "short" } }
                end
                return {}
            end
        "#;
        let mut config = test_config(script);
        config.market = "futures".into();
        config.position_mode = "hedge".into();
        // 关资金费避免结算污染 (本测试只验方向仓语义)。
        config.backtest = Some(crate::config::BacktestToml {
            funding_rate_8h: Some(0.0),
            ..Default::default()
        });
        let mut strategy = LuaStrategy::from_source(script, config).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );

        // tick1: 无 long → 开多 (position_side="long")。
        ctx.step_bar(sample_kline());
        let orders = strategy.on_tick(&mut ctx);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].position_side.as_deref(), Some("long"), "position_side 字段应透传");
        assert_eq!(orders[0].side, OrderSide::Buy);
        for req in orders {
            let _ = ctx.place_order(req);
        }
        ctx.drain_fills();

        // tick2: long=2 存在 → 开空 (position_side="short")。
        ctx.step_bar(sample_kline());
        let orders = strategy.on_tick(&mut ctx);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].position_side.as_deref(), Some("short"));
        assert_eq!(orders[0].side, OrderSide::Sell);
        for req in orders {
            let _ = ctx.place_order(req);
        }
        ctx.drain_fills();

        // hedge 双仓并存: long 2 + short 1 各自可见; 净仓 = 1 (多)。
        let long = ctx.position_directional("ETH", OrderSide::Buy).expect("多头仓应存在");
        let short = ctx.position_directional("ETH", OrderSide::Sell).expect("空头仓应存在");
        assert_eq!(long.size, dec!(2));
        assert_eq!(short.size, dec!(1));
        let net = ctx.position("ETH").expect("净仓应存在");
        assert_eq!(net.size, dec!(1), "净仓 = 多 2 − 空 1");
        assert_eq!(net.side, OrderSide::Buy);

        // tick3: 两侧都有仓 → 空订单。
        ctx.step_bar(sample_kline());
        assert!(strategy.on_tick(&mut ctx).is_empty());
    }

    /// T1 (bs_momentum Lua 化): BacktestContext::new resolve 后把最终撮合参数统一回写
    /// config.params, Lua 快照 config_f64 读到的与撮合层 FeeModel 同源 — 未显式配置的
    /// 默认值 (taker 10bps 等) 也应可读 (此前恒 0, 预算净回笼口径无法闭合)。
    #[test]
    fn test_fee_params_written_back_default_visible_to_lua() {
        let script = "function on_tick(ctx) return {} end";
        let strategy = LuaStrategy::from_source(script, test_config(script)).expect("编译应通过");
        let mut data = LuaCtxData::new();
        let ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        strategy.fill_snapshot(&ctx, &mut data);
        assert_eq!(data.config_f64("fee_taker_bps"), 10.0, "默认 taker 10bps 应可读");
        assert_eq!(data.config_f64("fee_maker_bps"), 10.0);
        assert_eq!(data.config_f64("slippage_bps"), 0.0, "滑点默认 0");
        assert_eq!(data.config_f64("leverage"), 1.0);
        assert_eq!(data.config_f64("mmr_pct"), 1.0);
        assert_eq!(data.config_f64("funding_rate_8h"), 0.0001);
    }

    /// T1: TOML [backtest] 段覆盖 → 回写覆盖值 (未覆盖键保持内置默认)。
    #[test]
    fn test_fee_params_written_back_toml_override_visible_to_lua() {
        let script = "function on_tick(ctx) return {} end";
        let mut config = test_config(script);
        config.backtest = Some(crate::config::BacktestToml {
            fee_taker_bps: Some(25.0),
            slippage_bps: Some(3.0),
            funding_rate_8h: Some(0.0),
            ..Default::default()
        });
        let strategy = LuaStrategy::from_source(script, config).expect("编译应通过");
        let mut data = LuaCtxData::new();
        let ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        strategy.fill_snapshot(&ctx, &mut data);
        assert_eq!(data.config_f64("fee_taker_bps"), 25.0, "[backtest] 覆盖值应回写");
        assert_eq!(data.config_f64("slippage_bps"), 3.0);
        assert_eq!(data.config_f64("funding_rate_8h"), 0.0);
        assert_eq!(data.config_f64("fee_maker_bps"), 10.0, "未覆盖键保持内置默认");
    }

    // ---- T3 (bs_momentum Lua 化): Lua 快照组合模式 ----

    /// 合成美股信号线: day 0 起每天一根 (UTC 00:00 锚), n 根, 价格 = base + i。
    fn t3_signal_line(n: usize, base: f64) -> Vec<Kline> {
        (0..n)
            .map(|i| {
                let t = i as i64 * 86_400;
                let p = Decimal::from_f64_retain(base + i as f64).unwrap();
                Kline {
                    open_time: chrono::DateTime::from_timestamp(t, 0).unwrap(),
                    open: p,
                    high: p + dec!(1),
                    low: p - dec!(1),
                    close: p,
                    volume: Decimal::ONE,
                    close_time: chrono::DateTime::from_timestamp(t + 86_400, 0).unwrap(),
                }
            })
            .collect()
    }

    /// 成交轨日线 bar (open_time = day 00:00 UTC)。
    fn t3_exec_bar(day: i64, px: i64) -> Kline {
        let t = day * 86_400;
        let p = Decimal::from(px);
        Kline {
            open_time: chrono::DateTime::from_timestamp(t, 0).unwrap(),
            open: p,
            high: p + dec!(1),
            low: p - dec!(1),
            close: p,
            volume: Decimal::ONE,
            close_time: chrono::DateTime::from_timestamp(t + 86_400, 0).unwrap(),
        }
    }

    /// T3: 组合信号模式 (universe 配置 + signal 预装) → Lua 脚本逐只读到全段信号线
    /// (≥253, 不套 100 cap; 300 根全可见 → add_method full 分支生效)。
    #[test]
    fn test_portfolio_universe_full_klines_in_lua() {
        let script = r#"
            seen = {}
            function on_tick(ctx)
                for _, p in ipairs({ "TSLABUSDT", "NVDABUSDT", "AAPLUSDT" }) do
                    local ks = ctx:klines(p)
                    if ks == nil then seen[p] = -1 else seen[p] = #ks end
                end
                return {}
            end
        "#;
        let mut config = test_config(script);
        config.params.insert(
            "universe".into(),
            ConfigValue::String("TSLABUSDT,NVDABUSDT,AAPLUSDT".into()),
        );
        let mut strategy = LuaStrategy::from_source(script, config).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut sig_map: HashMap<String, Vec<Kline>> = HashMap::new();
        for (pair, base) in [("TSLABUSDT", 100.0), ("NVDABUSDT", 200.0), ("AAPLUSDT", 300.0)] {
            sig_map.insert(pair.into(), t3_signal_line(300, base));
        }
        ctx.set_signal_klines(sig_map);
        // tick 推进到 day 400 → 截断层 = 全 300 根 (≥253)。
        ctx.step_portfolio(&[
            ("TSLABUSDT".into(), t3_exec_bar(400, 110)),
            ("NVDABUSDT".into(), t3_exec_bar(400, 210)),
            ("AAPLUSDT".into(), t3_exec_bar(400, 310)),
        ]);
        let _ = strategy.on_tick(&mut ctx);
        let seen = strategy.lua.globals().get::<mlua::Table>("seen").expect("seen 表");
        for pair in ["TSLABUSDT", "NVDABUSDT", "AAPLUSDT"] {
            let n: i64 = seen.get(pair).expect("该只应有记录");
            assert!(
                n >= 253,
                "{pair} 信号线应 ≥253 根 (full 模式, 不套 100 cap), got {n}"
            );
            assert_eq!(n, 300, "{pair} 300 根全段可见 (add_method full 分支)");
        }
    }

    /// T3 回归: 无 universe (单标的路径) → 脚本 ctx:klines 仍 ≤100 根 (行为边界)。
    #[test]
    fn test_single_klines_still_capped_at_100() {
        let script = r#"
            n = -1
            function on_tick(ctx)
                local ks = ctx:klines("ETH")
                if ks ~= nil then n = #ks end
                return {}
            end
        "#;
        let mut strategy = LuaStrategy::from_source(script, test_config(script)).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // 250 根 1h bar (单标的路径)。
        for i in 0..250u32 {
            let t = 5 * 3600 + i as i64 * 3600;
            let p = Decimal::from(3000 + i * 10);
            ctx.step_bar(Kline {
                open_time: chrono::DateTime::from_timestamp(t, 0).unwrap(),
                open: p,
                high: p + dec!(5),
                low: p - dec!(5),
                close: p + dec!(2),
                volume: Decimal::ONE,
                close_time: chrono::DateTime::from_timestamp(t + 3600, 0).unwrap(),
            });
        }
        let _ = strategy.on_tick(&mut ctx);
        let n: i64 = strategy.lua.globals().get("n").expect("n 已置位");
        assert_eq!(n, 100, "单标的 klines 仍 cap 100, got {n}");
    }
}
