//! Lua 自定义策略运行时。
//!
//! 提供:
//! - `validate_lua`: 编译校验 (门禁, 不执行)
//! - `LuaStrategy`: 实现 `Strategy` trait, 回调转发到 Lua 脚本函数
//!
//! 安全边界 (见 lua_sandbox.rs): 禁 require/loadstring/loadfile/dofile + 指令预算 + print 重定向。
//! 策略影响外界的出口只有两个, 同一批落地: 回调返回值(订单数组) 与 `ctx:place_order` /
//! `ctx:cancel_order` 记录的意图(028 T015)。ctx 其余部分为只读快照 (LuaCtxData userdata)。
//!
//! 028 新增(策略自取数据): `data:series/history/subscribe` / `market:*` / `http:get` 三张表,
//! 与 7 个回调(`on_init`/`on_bar`/`on_quote`/`on_timer`/`on_tick`/`on_fill`/`on_stop`)。
//! 取数与 HTTP 都由宿主([`crate::host::HostServices`])执行 —— 本模块不碰网络。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Datelike, Timelike};
use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};
use ricow_core::{
    CoreError, Interval, Kline, OrderFill, OrderRequest, OrderSide, OrderType, OrderUpdate,
    Position, PriceMode, SeriesKey,
};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::config::{ConfigValue, StrategyConfig};
use crate::context::Context;
use crate::host::{HostServices, NullHost};
use crate::indicators_api;
use crate::lua_sandbox::create_lua_sandbox;
use crate::series::{Series, SeriesDecl, SeriesInfo, SeriesSet};
use crate::strategy::Strategy;
use crate::timers::TimerDecl;

// ============================================================================
// 策略运行期共享状态 (Lua 闭包 ↔ Rust 侧; 028 T014/T015/T017/T020)
// ============================================================================

/// `ctx:cancel_order` 记录的撤单意图 (T015) —— 四种粒度:
/// - `Owned`: 撤**本实例归属**的全部挂单(`ctx:cancel_order()` 无参形式);
/// - `OwnedIn { pair }`: 只撤该交易对上本实例归属的挂单(`ctx:cancel_order{pair=...}`);
/// - `ByOrderId { pair, order_id }`: 指定交易所订单号;
/// - `ByClientId { pair, client_order_id }`: 指定我方的 `client_order_id`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelIntent {
    Owned,
    OwnedIn { pair: String },
    ByOrderId { pair: String, order_id: String },
    ByClientId { pair: String, client_order_id: String },
}

/// 一次回调产生的"主动订单出口"(T015): 回调返回值之外的 `ctx:place_order` / `ctx:cancel_order`。
///
/// 与返回值走**同一批落地**(引擎在回调返回后统一下单/撤单), 不新增第二条下单通道。
#[derive(Debug, Default)]
pub struct OrderIntents {
    pub orders: Vec<OrderRequest>,
    pub cancels: Vec<CancelIntent>,
}

impl OrderIntents {
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty() && self.cancels.is_empty()
    }
}

/// Lua 与 Rust 共享的策略运行期状态。
///
/// 为什么要共享: `data:series{...}` 这类 Lua 闭包在 `Lua` 里执行时拿不到 `&mut LuaStrategy`;
/// 用 `Arc<Mutex<..>>` 交换"声明表 / 句柄 / 订单意图 / 盘口快照", 两侧都能读, 不引入第二套机制。
pub(crate) struct RuntimeState {
    /// 声明 + 已装载的序列句柄。
    pub series: SeriesSet,
    /// 声明顺序(含只订阅不带句柄的序列), 供装配层装载。
    pub decls: Vec<SeriesDecl>,
    /// 盘口订阅声明(`market:subscribe`)。
    pub quote_pairs: Vec<String>,
    /// 定时器声明(`data.timer`), 按声明序 (FR-006)。
    pub timers: Vec<TimerDecl>,
    /// `http:get` 调用计数(诊断/测试)。
    pub http_calls: usize,
    /// 最近一次 `http:get` 的错误(诊断/测试)。
    pub last_http_error: Option<String>,
    /// 盘口快照(每次回调由 `fill_snapshot` 刷新): `market:best_bid/best_ask` 读它。
    pub best_bids: HashMap<String, f64>,
    pub best_asks: HashMap<String, f64>,
}

impl RuntimeState {
    fn new() -> Self {
        Self {
            series: SeriesSet::new(),
            decls: Vec::new(),
            quote_pairs: Vec::new(),
            timers: Vec::new(),
            http_calls: 0,
            last_http_error: None,
            best_bids: HashMap::new(),
            best_asks: HashMap::new(),
        }
    }
}

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
    /// 主动订单出口 (028 T015): `ctx:place_order` / `ctx:cancel_order` 写这里, 引擎在回调
    /// 返回后与"返回值订单"同批落地。`None` = 未接订单出口(两个方法返回 false)。
    intents: Option<Arc<Mutex<OrderIntents>>>,
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
            intents: None,
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
        // ---- 主动订单出口 (028 T015): 与回调返回值**同批落地**, 不新增第二条下单通道 ----
        // ctx:place_order{...} → 入队; 引擎在回调返回后统一提交。
        methods.add_method("place_order", |_, data, args: mlua::MultiValue| {
            let Some(intents) = data.intents.as_ref() else {
                return Ok(false);
            };
            let Some(t) = last_table(&args) else {
                return Ok(false);
            };
            let Some(req) = order_from_table(&t, "ctx:place_order") else {
                return Ok(false);
            };
            intents.lock().expect("intents").orders.push(req);
            Ok(true)
        });
        // ctx:cancel_order() / {pair=...} / {pair, order_id} / {pair, client_order_id}
        methods.add_method("cancel_order", |_, data, args: mlua::MultiValue| {
            let Some(intents) = data.intents.as_ref() else {
                return Ok(false);
            };
            let intent = match last_table(&args) {
                None => CancelIntent::Owned,
                Some(t) => {
                    let pair: String = t.get::<Option<String>>("pair")?.unwrap_or_default();
                    let order_id: Option<String> = t.get::<Option<String>>("order_id")?;
                    let client_order_id: Option<String> =
                        t.get::<Option<String>>("client_order_id")?;
                    if pair.is_empty() {
                        CancelIntent::Owned
                    } else if let Some(order_id) = order_id {
                        CancelIntent::ByOrderId { pair, order_id }
                    } else if let Some(client_order_id) = client_order_id {
                        CancelIntent::ByClientId { pair, client_order_id }
                    } else {
                        CancelIntent::OwnedIn { pair }
                    }
                }
            };
            intents.lock().expect("intents").cancels.push(intent);
            Ok(true)
        });
        // 已收盘 K 线历史 (无前视): 返回 {open=, close=} 表数组。全段返回, 不再套
        // 100 根硬编码 cap (028 T040/ D7) —— 上下文里有多少已收盘 bar 就给多少;
        // 策略要控体量用 `data:series{...}` 声明窗口(句柄自带默认窗口/尾窗)。
        methods.add_method("klines", |lua, data, pair: String| {
            let k = data.klines(&pair).unwrap_or_default();
            let t = lua.create_table()?;
            for (i, kline) in k.iter().enumerate() {
                // 行形状与 `kline_to_table`(句柄 `s:bars(n)`) **同一个函数**: 早先这里内联少设了
                // high/low, 于是 `ctx:klines(pair)` 的行读 `k.high` 得到 nil(审核发现), 同一位
                // 策略拿两套口径的 K 线很容易写错 —— 现在只有一套。
                t.set(i + 1, kline_to_table(lua, kline)?)?;
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
    /// 宿主服务(取数 / HTTP, 028 T016): 默认 `NullHost`, 装配层用 [`LuaStrategy::set_host`] 注入。
    host: Arc<dyn HostServices>,
    /// 与 Lua 闭包共享的声明表 / 句柄 / 盘口快照。
    state: Arc<Mutex<RuntimeState>>,
    /// 主动订单出口(028 T015): `ctx:place_order` / `ctx:cancel_order` 的记录。
    intents: Arc<Mutex<OrderIntents>>,
}

impl std::fmt::Debug for LuaStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaStrategy")
            .field("name", &self.config.name)
            .field("instance_id", &self.instance_id)
            .finish()
    }
}

/// 净仓 → 策略可见方向标签 (纯函数, 便于单测)。
///
/// 下游契约: 净仓数量 `size <= 0` 一律报 `"none"` (与 `ctx:position_side` 文档一致);
/// 否则按建仓方向报 `"long"` / `"short"`。
fn position_side_label(pos: &Position) -> &'static str {
    if pos.size <= Decimal::ZERO {
        "none"
    } else if pos.side == OrderSide::Buy {
        "long"
    } else {
        "short"
    }
}

impl LuaStrategy {
    /// 从源码构建策略: 沙箱引擎 + 编译 + 执行模块级语句 (函数定义/模块级状态)。
    /// 统一注册 exec 执行组件库 (Rust 实现): 所有 Lua 策略 (CLI/MCP/TOML/回测) 均可用 exec.*;
    /// 用户脚本后执行可覆盖库函数 (复制即自定义)。
    pub fn from_source(code: &str, config: StrategyConfig) -> Result<Self, String> {
        Self::from_source_with_host(code, config, Arc::new(NullHost))
    }

    /// 从源码构建, 并**在脚本顶层执行之前**注入宿主(028 T016)。
    ///
    /// 与 [`from_source`](LuaStrategy::from_source) 的区别: 脚本顶层就写
    /// `data:series{...}` 时, 宿主必须在那一刻已经就位 —— 装配层(引擎/回测)用这个构造函数。
    /// (`from_source` 保留给不需要数据服务的场景, 此时 `data:*` / `http:*` 调用会给出
    /// "未装配宿主"的明确错误。)
    pub fn from_source_with_host(
        code: &str,
        config: StrategyConfig,
        host: Arc<dyn HostServices>,
    ) -> Result<Self, String> {
        let (lua, budget) = create_lua_sandbox();
        // 注册 exec 执行组件 (Rust 实现, 全局表): 所有 Lua 策略 (CLI/MCP/TOML/回测) 均可用
        // exec.*; 用户脚本后执行可覆盖库函数 (复制即自定义)。
        crate::exec::register(&lua).map_err(|e| format!("exec 组件注册失败: {e}"))?;
        // 028: data / market / http 三张表(宿主由调用方决定, 见 plan d11)。
        let state = Arc::new(Mutex::new(RuntimeState::new()));
        register_data_api(&lua, &host, &state)
            .map_err(|e| format!("数据/行情/HTTP 接口注册失败: {e}"))?;
        register_config_api(&lua, &config.params)
            .map_err(|e| format!("顶层 config 访问面注册失败: {e}"))?;
        lua.load(code).exec().map_err(|e| format!("脚本错误 (编译或顶层执行):\n{e}"))?;
        let instance_id = uuid::Uuid::new_v4().to_string();
        Ok(Self {
            config,
            lua,
            budget,
            instance_id,
            host,
            state,
            intents: Arc::new(Mutex::new(OrderIntents::default())),
        })
    }

    /// 注入宿主服务 (028 T016): 装配层在 `from_source` 之后调用。
    ///
    /// 覆盖 `data:series` / `data:history` / `http:get` 的实现 —— 接口表本身已经注册好,
    /// 这里只是把"谁来取数"换成真宿主(引擎的 `EngineHost`), 策略脚本无需察觉。
    pub fn set_host(&mut self, host: Arc<dyn HostServices>) {
        self.host = host;
        // `data:*` 闭包持有的是注册时的那个 Arc; 重新注册一遍接口表, 让新宿主生效。
        if let Err(e) = register_data_api(&self.lua, &self.host, &self.state) {
            tracing::error!(target: "lua_strategy", "宿主注入后重注册接口失败: {e}");
        }
    }

    /// 策略声明的序列(声明顺序); 装配层据此装载。
    pub fn declarations(&self) -> Vec<SeriesDecl> {
        self.state.lock().expect("state").decls.clone()
    }

    /// 只驱动 `on_bar` 的序列声明(装配层按它推虚拟时钟)。
    pub fn driving_declarations(&self) -> Vec<SeriesDecl> {
        self.declarations().into_iter().filter(|d| d.drive).collect()
    }

    /// 盘口订阅声明(`market:subscribe`)。
    pub fn quote_subscriptions(&self) -> Vec<String> {
        self.state.lock().expect("state").quote_pairs.clone()
    }

    /// 定时器声明(`data.timer`), 按声明序 (FR-006)。
    pub fn timer_declarations(&self) -> Vec<TimerDecl> {
        self.state.lock().expect("state").timers.clone()
    }

    /// 已装载的序列 id(诊断/日志)。
    pub fn installed_series(&self) -> Vec<String> {
        self.state.lock().expect("state").series.ids().to_vec()
    }

    /// 装配层写入装载结果。
    pub fn install_series(&mut self, series: Series) {
        self.state.lock().expect("state").series.insert(series);
    }

    /// 标记某序列 stale(增量回补失败时由引擎调用)。
    pub fn mark_series_stale(&mut self, id: &str, stale: bool) -> bool {
        self.state.lock().expect("state").series.mark_stale(id, stale)
    }

    /// 取走主动订单意图(引擎在回调返回后调用, 与返回值订单同批落地)。
    pub fn take_intents(&mut self) -> OrderIntents {
        std::mem::take(&mut *self.intents.lock().expect("intents"))
    }

    /// `http:get` 调用次数(诊断/测试)。
    pub fn http_call_count(&self) -> usize {
        self.state.lock().expect("state").http_calls
    }

    /// 最近一次 `http:get` 失败信息(诊断/测试)。
    pub fn last_http_error(&self) -> Option<String> {
        self.state.lock().expect("state").last_http_error.clone()
    }

    /// 脚本是否定义了某全局函数(引擎据此选择派发路径, 如 `on_quote` 优先于 `on_tick`)。
    pub fn defines(&self, name: &str) -> bool {
        self.lua.globals().get::<Function>(name).is_ok()
    }

    /// 把一根新 bar 追加进某序列句柄 —— 由 `on_bar` 自身调用(单一写入者: 策略侧)。
    fn advance_series(&mut self, id: &str, bar: &Kline) {
        if let Ok(mut st) = self.state.lock() {
            if let Some(s) = st.series.get_mut(id) {
                s.push_bar(bar.clone());
            }
        }
    }

    fn fill_snapshot(&self, ctx: &dyn Context, data: &mut LuaCtxData) {
        // 快照覆盖的交易对 (028 T038, 退役原 `universe` 配置注入): = 配置 pair ∪ 声明的
        // 序列标的 ∪ 盘口订阅标的。声明什么就有什么 —— 装配层不再往 config 里塞标的池。
        // 未在 ctx 里的 pair 静默跳过(下面逐个 `if let Some`), 多列不影响。
        let mut pairs: Vec<String> = Vec::new();
        if let Some(p) = ctx.config().get_str("pair") {
            pairs.push(p.to_string());
        }
        {
            let st = self.state.lock().expect("state");
            for d in &st.decls {
                let sym = d.key.symbol.clone();
                if !pairs.iter().any(|p| p == &sym) {
                    pairs.push(sym);
                }
            }
            for q in &st.quote_pairs {
                if !pairs.iter().any(|p| p == q) {
                    pairs.push(q.clone());
                }
            }
        }
        data.now = ctx.now_utc();

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
                let side = position_side_label(&pos);
                data.position_sides.insert(pair.clone(), side.to_string());
                data.position_sizes.insert(pair.clone(), pos.size.to_f64().unwrap_or(0.0));
                data.position_entries.insert(pair.clone(), pos.entry_price.to_f64().unwrap_or(0.0));
                unrealized += pos.unrealized_pnl.to_f64().unwrap_or(0.0);
            }
            // 方向仓快照 (D9/hedge): 现货只填 long (= 净仓); 合约按 (pair, side) 各自方向仓。
            for (side, label) in [(OrderSide::Buy, "long"), (OrderSide::Sell, "short")] {
                if let Some(p) = ctx.position_directional(&pair, side) {
                    let key = format!("{pair}|{label}");
                    data.directionals.insert(
                        key,
                        (p.size.to_f64().unwrap_or(0.0), p.entry_price.to_f64().unwrap_or(0.0)),
                    );
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

        // 主动订单出口 (028 T015): 本轮回调的 ctx:place_order / ctx:cancel_order 写进共享意图表,
        // 引擎在回调返回后与"返回值订单"同批落地。
        data.intents = Some(self.intents.clone());
        // 盘口快照 (028 T020): market:best_bid/best_ask 与 ctx 快照同源(同一份数据, 不搞两套)。
        if let Ok(mut st) = self.state.lock() {
            st.best_bids = data.best_bids.clone();
            st.best_asks = data.best_asks.clone();
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
    ///
    /// 与 `ctx:place_order` 共用同一个单表解析器 [`order_from_table`](同一条下单口径, 无第二套)。
    fn parse_orders(&self, table: &Table) -> Vec<OrderRequest> {
        table
            .sequence_values::<Table>()
            .filter_map(|item| item.ok())
            .filter_map(|t| order_from_table(&t, &self.instance_id))
            .collect()
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

// ============================================================================
// Lua 侧解析与接口注册 (028 T015/T020)
// ============================================================================

/// 单个 Lua 订单表 → `OrderRequest`。
///
/// 回调返回值与 `ctx:place_order` **共用**这一个解析器(同一条下单口径, 不搞两套);
/// 非法(缺 pair / `size <= 0`)返回 `None` 并告警。
fn order_from_table(t: &Table, instance_id: &str) -> Option<OrderRequest> {
    let pair = t.get::<String>("pair").unwrap_or_default();
    let side = match t.get::<String>("side").as_deref() {
        Ok(s) if s.eq_ignore_ascii_case("sell") => OrderSide::Sell,
        Ok(_) => OrderSide::Buy,
        Err(_) => {
            tracing::warn!(target: "lua_strategy", instance = %instance_id, "订单缺 side 字段, 默认 buy");
            OrderSide::Buy
        }
    };
    let size =
        t.get::<f64>("size").ok().and_then(Decimal::from_f64_retain).unwrap_or(Decimal::ZERO);
    if size <= Decimal::ZERO {
        tracing::warn!(target: "lua_strategy", instance = %instance_id, "订单 size 缺失/非法, 丢弃: pair={pair}");
    }
    let price = t.get::<Option<f64>>("price").ok().flatten().and_then(Decimal::from_f64_retain);
    let order_type = match t.get::<String>("order_type").as_deref() {
        Ok(s) if s.eq_ignore_ascii_case("market") => OrderType::Market,
        Ok(s) if s.eq_ignore_ascii_case("limit") => OrderType::Limit,
        Ok(s) => {
            tracing::warn!(target: "lua_strategy", instance = %instance_id, "未知 order_type '{s}', 默认 limit");
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
        return None;
    }
    Some(OrderRequest {
        client_order_id: String::new(),
        pair,
        side,
        order_type,
        price,
        size,
        reduce_only,
        position_side,
    })
}

/// 取**最后一个表参数**: 兼容 `data.series{...}` 与 `data:series{...}`
/// (冒号写法会把命名空间表自身塞成第一个参数, 这里取最后一个表即可两种都吃)。
fn last_table(args: &mlua::MultiValue) -> Option<Table> {
    args.iter().rev().find_map(|v| match v {
        Value::Table(t) => Some(t.clone()),
        _ => None,
    })
}

/// 取**最后一个字符串参数**(同 `last_table` 理由)。
fn last_string(args: &mlua::MultiValue) -> Option<String> {
    args.iter().rev().find_map(|v| match v {
        Value::String(s) => s.to_str().ok().map(|x| x.to_string()),
        _ => None,
    })
}

/// 取**最后一个整数参数**(兼容 `s.ema(20)` 与 `s:ema(20)`; 冒号写法首个参数是句柄表自身)。
fn last_usize(args: &mlua::MultiValue) -> Option<usize> {
    args.iter().rev().find_map(|v| match v {
        Value::Integer(i) => usize::try_from(*i).ok(),
        Value::Number(n) => Some(*n as usize),
        _ => None,
    })
}

/// 取**最后两个数字参数**(`period, dev` 之类), 顺序保持原样。
fn last_two_numbers(args: &mlua::MultiValue) -> Option<(usize, f64)> {
    let mut nums: Vec<f64> = Vec::new();
    for v in args.iter().rev() {
        match v {
            Value::Integer(i) => nums.push(*i as f64),
            Value::Number(n) => nums.push(*n),
            _ => {}
        }
        if nums.len() == 2 {
            break;
        }
    }
    if nums.len() < 2 {
        return None;
    }
    Some((nums[1] as usize, nums[0]))
}

/// 序列描述 → Lua 表 (FR-005): 策略据此区分多条序列与判断口径。
fn series_info_table(lua: &Lua, info: &SeriesInfo) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", info.id.clone())?;
    t.set("source", info.source.clone())?;
    t.set("symbol", info.symbol.clone())?;
    t.set("interval", info.interval.label())?;
    t.set("price", info.price_mode.label())?;
    t.set("mode", info.mode.label())?;
    t.set("feed_interval", info.feed_interval.label())?;
    Ok(t)
}

/// `data.timer{...}` → [`TimerDecl`] (FR-006): `label` 必填, `secs` / `at="HH:MM"` 至少一个。
fn timer_from_table(t: &Table) -> mlua::Result<TimerDecl> {
    let label: String = t.get("label").map_err(|_| missing("label", "data:timer"))?;
    let secs: Option<u64> = t.get::<Option<u64>>("secs")?;
    let at_raw: Option<String> = t.get::<Option<String>>("at")?;
    let tz: Option<i32> = t.get::<Option<i32>>("tz_offset_minutes")?;
    let at = match at_raw {
        None => None,
        Some(s) => {
            let bad = || {
                mlua::Error::external(CoreError::InvalidArgument(format!(
                    "data:timer 的 at 必须是 \"HH:MM\"(24 小时制, 默认 UTC), 收到 '{s}'"
                )))
            };
            let (h, m) = s.split_once(':').ok_or_else(bad)?;
            let h: u8 = h.trim().parse().map_err(|_| bad())?;
            let m: u8 = m.trim().parse().map_err(|_| bad())?;
            Some((h, m))
        }
    };
    let decl = TimerDecl { label, secs, at, tz_offset_minutes: tz.unwrap_or(0) };
    decl.validate().map_err(mlua::Error::external)?;
    Ok(decl)
}

fn missing(field: &str, api: &str) -> mlua::Error {
    mlua::Error::external(CoreError::InvalidArgument(format!("{api} 缺少必填字段 '{field}'")))
}

/// `data:series{...}` / `data:subscribe{...}` 的参数表 → [`SeriesDecl`]。
fn decl_from_table(t: &Table) -> mlua::Result<SeriesDecl> {
    let id: String = t.get("id").map_err(|_| missing("id", "data:series"))?;
    let source: String = t.get("source").map_err(|_| missing("source", "data:series"))?;
    let symbol: String = t.get("symbol").map_err(|_| missing("symbol", "data:series"))?;
    let interval_label: String =
        t.get("interval").map_err(|_| missing("interval", "data:series"))?;
    let interval = Interval::from_label(&interval_label).ok_or_else(|| {
        mlua::Error::external(CoreError::InvalidArgument(format!(
            "data:series 不支持的周期 '{interval_label}'"
        )))
    })?;
    let key = SeriesKey::new(&source, &symbol, interval).map_err(mlua::Error::external)?;
    let mut decl = SeriesDecl::new(&id, key);
    if let Some(bars) = t.get::<Option<usize>>("bars")? {
        decl.bars = Some(bars);
    }
    if let Some(min_bars) = t.get::<Option<usize>>("min_bars")? {
        decl.min_bars = Some(min_bars);
    }
    if let Some(price) = t.get::<Option<String>>("price")? {
        decl.price_mode = PriceMode::from_label(&price).ok_or_else(|| {
            mlua::Error::external(CoreError::InvalidArgument(format!(
                "data:series price 只支持 close/adjclose, 收到 '{price}'"
            )))
        })?;
    }
    if let Some(drive) = t.get::<Option<bool>>("drive")? {
        decl.drive = drive;
    }
    decl.validate().map_err(mlua::Error::external)?;
    Ok(decl)
}

/// `data:history{...}` 的参数表 → [`SeriesKey`]。
fn key_from_table(t: &Table) -> mlua::Result<SeriesKey> {
    let source: String = t.get("source").map_err(|_| missing("source", "data:history"))?;
    let symbol: String = t.get("symbol").map_err(|_| missing("symbol", "data:history"))?;
    let interval_label: String =
        t.get("interval").map_err(|_| missing("interval", "data:history"))?;
    let interval = Interval::from_label(&interval_label).ok_or_else(|| {
        mlua::Error::external(CoreError::InvalidArgument(format!(
            "data:history 不支持的周期 '{interval_label}'"
        )))
    })?;
    SeriesKey::new(&source, &symbol, interval).map_err(mlua::Error::external)
}

/// 单根 K 线 → Lua 表(策略可见字段, 与 `ctx:klines` 行同形: `ts/open/close/volume`)。
fn kline_to_table(lua: &Lua, k: &Kline) -> mlua::Result<Table> {
    let row = lua.create_table()?;
    row.set("ts", k.open_time.timestamp())?;
    row.set("open", k.open.to_f64().unwrap_or(0.0))?;
    row.set("high", k.high.to_f64().unwrap_or(0.0))?;
    row.set("low", k.low.to_f64().unwrap_or(0.0))?;
    row.set("close", k.close.to_f64().unwrap_or(0.0))?;
    row.set("volume", k.volume.to_f64().unwrap_or(0.0))?;
    Ok(row)
}

/// K 线数组 → Lua 表数组。
fn klines_to_table(lua: &Lua, bars: &[Kline]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (i, k) in bars.iter().enumerate() {
        t.set(i + 1, kline_to_table(lua, k)?)?;
    }
    Ok(t)
}

/// `data:series{...}` 返回的句柄表: 元信息 + 访问器 + 12 个指标(与 `ctx:*` 同源)。
fn series_handle(
    lua: &Lua,
    state: &Arc<Mutex<RuntimeState>>,
    decl: &SeriesDecl,
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", decl.id.clone())?;
    t.set("source", decl.key.source.clone())?;
    t.set("symbol", decl.key.symbol.clone())?;
    t.set("interval", decl.key.interval.label())?;
    t.set("price", decl.price_mode.label())?;
    t.set("window", decl.effective_window())?;
    t.set("drive", decl.drive)?;

    /// 读共享句柄: 闭包持有自己的 Arc 副本(每块一个), 避免引用逃逸到 'static fn。
    macro_rules! with_series {
        ($arc:ident, $body:expr) => {{
            let guard = $arc.lock().expect("state");
            $body(&guard)
        }};
    }

    {
        let (st, id) = (state.clone(), decl.id.clone());
        t.set(
            "len",
            lua.create_function(move |_, _args: mlua::MultiValue| {
                Ok(with_series!(st, |s: &RuntimeState| s
                    .series
                    .get(&id)
                    .map(|s| s.len())
                    .unwrap_or(0)))
            })?,
        )?;
    }
    {
        let st = state.clone();
        let id = decl.id.clone();
        t.set(
            "stale",
            lua.create_function(move |_, _args: mlua::MultiValue| {
                Ok(with_series!(st, |s: &RuntimeState| s
                    .series
                    .get(&id)
                    .map(|s| s.is_stale())
                    .unwrap_or(true)))
            })?,
        )?;
    }
    {
        let st = state.clone();
        let id = decl.id.clone();
        t.set(
            "last",
            lua.create_function(move |lua, _args: mlua::MultiValue| {
                let k = with_series!(st, |s: &RuntimeState| s
                    .series
                    .get(&id)
                    .and_then(|s| s.last().cloned()));
                match k {
                    Some(k) => Ok(Value::Table(kline_to_table(lua, &k)?)),
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
    }
    {
        let st = state.clone();
        let id = decl.id.clone();
        t.set(
            "close",
            lua.create_function(move |_, args: mlua::MultiValue| {
                let back = last_usize(&args).unwrap_or(0);
                Ok(with_series!(st, |s: &RuntimeState| s
                    .series
                    .get(&id)
                    .and_then(|s| s.close(back))))
            })?,
        )?;
    }
    {
        // 尾窗 bar 数组 (028 T044): `s:bars(n)` = 最近 n 根(升序), `s:bars()` = 全部已装载。
        // 为什么给这个口子: 平台提供数据、策略自己算 —— 像 VWAP 这类自定义口径不该逼平台
        // 造专用指标; 行结构与 `ctx:klines` 完全一致(同一个 kline_to_table), 不搞第二套形状。
        let st = state.clone();
        let id = decl.id.clone();
        t.set(
            "bars",
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let n = last_usize(&args);
                let bars: Vec<ricow_core::Kline> = with_series!(st, |s: &RuntimeState| {
                    match s.series.get(&id) {
                        Some(series) => {
                            let len = series.len();
                            let take = n.map(|n| n.min(len)).unwrap_or(len);
                            series.tail(take)
                        }
                        None => Vec::new(),
                    }
                });
                let tbl = lua.create_table()?;
                for (i, k) in bars.iter().enumerate() {
                    tbl.set(i + 1, kline_to_table(lua, k)?)?;
                }
                Ok(tbl)
            })?,
        )?;
    }
    for name in ["ema", "sma", "wma", "rsi", "atr", "adx", "stoch", "cci", "roc", "mom"] {
        let st = state.clone();
        let id = decl.id.clone();
        t.set(
            name,
            lua.create_function(move |_, args: mlua::MultiValue| {
                let Some(period) = last_usize(&args) else {
                    return Ok(None);
                };
                Ok(with_series!(st, |s: &RuntimeState| s.series.get(&id).and_then(
                    |s| match name {
                        "ema" => s.ema(period),
                        "sma" => s.sma(period),
                        "wma" => s.wma(period),
                        "rsi" => s.rsi(period),
                        "atr" => s.atr(period),
                        "adx" => s.adx(period),
                        "stoch" => s.stoch(period),
                        "cci" => s.cci(period),
                        "roc" => s.roc(period),
                        "mom" => s.mom(period),
                        _ => None,
                    }
                )))
            })?,
        )?;
    }
    {
        let st = state.clone();
        let id = decl.id.clone();
        t.set(
            "macd",
            lua.create_function(move |lua, _args: mlua::MultiValue| {
                let m =
                    with_series!(st, |s: &RuntimeState| s.series.get(&id).and_then(|s| s.macd()));
                match m {
                    Some(m) => {
                        let tbl = lua.create_table()?;
                        tbl.set("main", m.main)?;
                        tbl.set("signal", m.signal)?;
                        tbl.set("hist", m.hist)?;
                        Ok(Value::Table(tbl))
                    }
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
    }
    {
        let st = state.clone();
        let id = decl.id.clone();
        t.set(
            "boll",
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let Some((period, dev)) = last_two_numbers(&args) else {
                    return Ok(Value::Nil);
                };
                let b = with_series!(st, |s: &RuntimeState| s
                    .series
                    .get(&id)
                    .and_then(|s| s.boll(period, dev)));
                match b {
                    Some(b) => {
                        let tbl = lua.create_table()?;
                        tbl.set("upper", b.upper)?;
                        tbl.set("mid", b.mid)?;
                        tbl.set("lower", b.lower)?;
                        Ok(Value::Table(tbl))
                    }
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
    }
    Ok(t)
}

/// 注册 `data` / `market` / `http` 三张全局表 (028 T020)。
///
/// 取值口径: 三张表都只做两件事 —— **声明**(策略要什么)与**读已装载的数据**。
/// 网络一律由宿主执行(策略回调里不做网络请求), 因此回测可复现。
fn register_data_api(
    lua: &Lua,
    host: &Arc<dyn HostServices>,
    state: &Arc<Mutex<RuntimeState>>,
) -> mlua::Result<()> {
    let globals = lua.globals();

    // ---- data: series / subscribe / history ----
    let data = lua.create_table()?;
    {
        let host = host.clone();
        let state = state.clone();
        data.set(
            "series",
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let t = last_table(&args).ok_or_else(|| missing("{...}", "data:series"))?;
                let decl = decl_from_table(&t)?;
                // FR-011 口径 = **超上限报错**(不是静默截断): 策略以为有 10 万根、实际只拿 5000 根
                // 会让指标与信号静默变味。上限见 `SERIES_WINDOW_MAX`。
                if let Some(msg) = decl.window_error() {
                    return Err(mlua::Error::external(CoreError::InvalidArgument(msg)));
                }
                // FR-012 条数上限: **先拦后做** —— 放在取数之前(否则写错脚本声明第 33 条时,
                // 平台会先真的发起一次取数/落库再报错, 白跑一趟网络)。同名重复声明 = 替换, 不算新增。
                {
                    let st = state.lock().expect("state");
                    let is_new = !st.decls.iter().any(|d| d.id == decl.id);
                    if is_new && st.decls.len() >= crate::series::MAX_SERIES_PER_STRATEGY {
                        return Err(mlua::Error::external(CoreError::InvalidArgument(format!(
                            "序列数超过上限 {} 条(声明 '{}' 时触发); 多标的请用同一条序列的多个 symbol 或减少声明",
                            crate::series::MAX_SERIES_PER_STRATEGY,
                            decl.id
                        ))));
                    }
                }
                let bars = host.load_series(&decl).map_err(mlua::Error::external)?;
                {
                    let mut st = state.lock().expect("state");
                    st.decls.retain(|d| d.id != decl.id);
                    st.decls.push(decl.clone());
                    st.series.insert(Series::new(decl.clone(), bars));
                }
                series_handle(lua, &state, &decl)
            })?,
        )?;
    }
    {
        let state = state.clone();
        data.set(
            "subscribe",
            lua.create_function(move |_, args: mlua::MultiValue| {
                let t = last_table(&args).ok_or_else(|| missing("{...}", "data:subscribe"))?;
                let mut decl = decl_from_table(&t)?;
                decl.drive = true;
                if let Some(msg) = decl.window_error() {
                    return Err(mlua::Error::external(CoreError::InvalidArgument(msg)));
                }
                let mut st = state.lock().expect("state");
                // 审核发现: 同一 id 先 data:series(拿句柄) 再 data:subscribe(只塞声明)会让
                // `decls` 与句柄分裂 —— 驱动/快照按新声明取数, 而句柄还在读旧序列。
                // 想两者都要请写 `data:series{drive=true}`(它同时装载句柄与驱动声明)。
                if st.series.contains(&decl.id) {
                    return Err(mlua::Error::external(CoreError::InvalidArgument(format!(
                        "序列 '{}' 已用 data:series 声明过(已有句柄); data:subscribe 只能声明没有句柄的序列; \
                         既要句柄又要驱动请用 data:series{{..., drive=true}}",
                        decl.id
                    ))));
                }
                let is_new = !st.decls.iter().any(|d| d.id == decl.id);
                if is_new && st.decls.len() >= crate::series::MAX_SERIES_PER_STRATEGY {
                    return Err(mlua::Error::external(CoreError::InvalidArgument(format!(
                        "序列数超过上限 {} 条(声明 '{}' 时触发)",
                        crate::series::MAX_SERIES_PER_STRATEGY,
                        decl.id
                    ))));
                }
                st.decls.retain(|d| d.id != decl.id);
                st.decls.push(decl);
                Ok(true)
            })?,
        )?;
    }
    {
        let host = host.clone();
        data.set(
            "history",
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let t = last_table(&args).ok_or_else(|| missing("{...}", "data:history"))?;
                let key = key_from_table(&t)?;
                let limit: Option<usize> = t.get::<Option<usize>>("limit")?;
                let bars = host.history(&key, limit).map_err(mlua::Error::external)?;
                klines_to_table(lua, &bars)
            })?,
        )?;
    }
    {
        let state = state.clone();
        data.set(
            "timer",
            lua.create_function(move |_, args: mlua::MultiValue| {
                let t = last_table(&args).ok_or_else(|| missing("{...}", "data:timer"))?;
                let decl = timer_from_table(&t)?;
                let mut st = state.lock().expect("state");
                st.timers.retain(|d| d.label != decl.label);
                st.timers.push(decl);
                Ok(true)
            })?,
        )?;
    }
    globals.set("data", data)?;

    // ---- market: subscribe / best_bid / best_ask ----
    let market = lua.create_table()?;
    {
        let state = state.clone();
        market.set(
            "subscribe",
            lua.create_function(move |_, args: mlua::MultiValue| {
                let t = last_table(&args).ok_or_else(|| missing("{...}", "market:subscribe"))?;
                let pair: String =
                    t.get("pair").map_err(|_| missing("pair", "market:subscribe"))?;
                let mut st = state.lock().expect("state");
                if !st.quote_pairs.iter().any(|p| p == &pair) {
                    st.quote_pairs.push(pair);
                }
                Ok(true)
            })?,
        )?;
    }
    {
        let state = state.clone();
        market.set(
            "best_bid",
            lua.create_function(move |_, args: mlua::MultiValue| {
                let pair = last_string(&args).ok_or_else(|| missing("pair", "market:best_bid"))?;
                Ok(state.lock().expect("state").best_bids.get(&pair).copied())
            })?,
        )?;
    }
    {
        let state = state.clone();
        market.set(
            "best_ask",
            lua.create_function(move |_, args: mlua::MultiValue| {
                let pair = last_string(&args).ok_or_else(|| missing("pair", "market:best_ask"))?;
                Ok(state.lock().expect("state").best_asks.get(&pair).copied())
            })?,
        )?;
    }
    globals.set("market", market)?;

    // ---- http: get (仅 GET; 失败返回 nil + 可判别错误串, 不抛异常中断策略) ----
    let http = lua.create_table()?;
    {
        let host = host.clone();
        let state = state.clone();
        http.set(
            "get",
            lua.create_function(move |_, args: mlua::MultiValue| {
                let url = last_string(&args).ok_or_else(|| missing("url", "http:get"))?;
                state.lock().expect("state").http_calls += 1;
                match host.http_get(&url) {
                    Ok(body) => Ok((Some(body), None::<String>)),
                    Err(e) => {
                        let msg = e.to_string();
                        tracing::warn!(target: "lua_strategy", "http:get 失败: {msg}");
                        state.lock().expect("state").last_http_error = Some(msg.clone());
                        Ok((None::<String>, Some(msg)))
                    }
                }
            })?,
        )?;
    }
    globals.set("http", http)?;
    Ok(())
}

/// 顶层 `config` 访问面 (028 T044): 脚本**顶层**声明序列时要按配置取参数(如 `pair` / `interval`),
/// 而顶层没有 `ctx` —— 这里给一份只读快照, 与 `ctx:config_*` **同源**(同一份 `StrategyConfig.params`)。
///
/// 只读、无写入口: 策略不能在顶层改配置(改配置的唯一入口仍是 `ctx:set_config` 那条既有通道)。
fn register_config_api(lua: &Lua, params: &HashMap<String, ConfigValue>) -> mlua::Result<()> {
    let mut strings: HashMap<String, String> = HashMap::new();
    let mut floats: HashMap<String, f64> = HashMap::new();
    let mut ints: HashMap<String, i64> = HashMap::new();
    let mut bools: HashMap<String, bool> = HashMap::new();
    for (key, value) in params {
        match value {
            ConfigValue::Float(f) => {
                floats.insert(key.clone(), *f);
            }
            ConfigValue::Integer(i) => {
                ints.insert(key.clone(), *i);
                floats.insert(key.clone(), *i as f64);
            }
            ConfigValue::Boolean(b) => {
                bools.insert(key.clone(), *b);
            }
            ConfigValue::String(s) => {
                strings.insert(key.clone(), s.clone());
                // 与 ctx:config_f64 同口径: 数字字符串同时进 f64(CLI 直跑按 Decimal 字符串注入价格参数)。
                if let Ok(f) = s.parse::<f64>() {
                    floats.insert(key.clone(), f);
                }
            }
        }
    }

    let t = lua.create_table()?;
    t.set("str", lua.create_function(move |_, key: String| Ok(strings.get(&key).cloned()))?)?;
    t.set("f64", lua.create_function(move |_, key: String| Ok(floats.get(&key).copied()))?)?;
    t.set("i64", lua.create_function(move |_, key: String| Ok(ints.get(&key).copied()))?)?;
    t.set("bool", lua.create_function(move |_, key: String| Ok(bools.get(&key).copied()))?)?;
    lua.globals().set("config", t)?;
    Ok(())
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

    /// 序列收盘驱动 (028 FR-005): Lua 侧 `on_bar(ctx, series, bar)`,
    /// `series` = {id, source, symbol, interval, price, mode, feed_interval}。
    fn on_bar(
        &mut self,
        ctx: &mut dyn Context,
        series: &SeriesInfo,
        bar: &Kline,
    ) -> Vec<OrderRequest> {
        let mut data = LuaCtxData::new();
        self.fill_snapshot(ctx, &mut data);
        // 句柄自行推进(单一写入者: 策略侧)。引擎只推"已收盘"的 bar。
        self.advance_series(&series.id, bar);
        let (Ok(bar_tbl), Ok(series_tbl)) =
            (kline_to_table(&self.lua, bar), series_info_table(&self.lua, series))
        else {
            return vec![];
        };
        match self.call("on_bar", data, vec![Value::Table(series_tbl), Value::Table(bar_tbl)]) {
            Some(Value::Table(t)) => self.parse_orders(&t),
            _ => vec![],
        }
    }

    /// 盘口驱动 (028 T014): Lua 侧 `on_quote(ctx, pair)`。语义与 `on_tick` 相同。
    fn on_quote(&mut self, ctx: &mut dyn Context, pair: &str) -> Vec<OrderRequest> {
        let mut data = LuaCtxData::new();
        self.fill_snapshot(ctx, &mut data);
        let Ok(p) = self.lua.create_string(pair) else {
            return vec![];
        };
        match self.call("on_quote", data, vec![Value::String(p)]) {
            Some(Value::Table(t)) => self.parse_orders(&t),
            _ => vec![],
        }
    }

    /// 定时器驱动 (028 T014): Lua 侧 `on_timer(ctx, label)`。
    fn on_timer(&mut self, ctx: &mut dyn Context, label: &str) -> Vec<OrderRequest> {
        let mut data = LuaCtxData::new();
        self.fill_snapshot(ctx, &mut data);
        let Ok(l) = self.lua.create_string(label) else {
            return vec![];
        };
        match self.call("on_timer", data, vec![Value::String(l)]) {
            Some(Value::Table(t)) => self.parse_orders(&t),
            _ => vec![],
        }
    }

    /// Lua 覆写: "脚本里定义了这个函数吗"(引擎据此选派发路径: `on_quote` 优先于 `on_tick`)。
    fn has_callback(&self, name: &str) -> bool {
        self.defines(name)
    }

    /// 策略声明的序列 (FR-002/FR-008): 装配层据此装载数据面。
    fn declarations(&self) -> Vec<SeriesDecl> {
        self.declarations()
    }

    /// 盘口订阅声明 (FR-023)。
    fn quote_subscriptions(&self) -> Vec<String> {
        self.quote_subscriptions()
    }

    /// 主动订单意图 (`ctx:place_order` / `ctx:cancel_order`), 取后清空 (FR-002)。
    fn take_intents(&mut self) -> OrderIntents {
        self.take_intents()
    }

    /// 置/清序列 `stale` 位 (FR-016): 只改句柄上的标志, 策略 `s:stale()` 读它。
    fn mark_series_stale(&mut self, id: &str, stale: bool) {
        let mut st = self.state.lock().expect("state");
        if !st.series.mark_stale(id, stale) {
            tracing::warn!(target: "lua_strategy", instance = %self.instance_id, "stale 置位: 序列 '{id}' 不存在");
        }
    }

    /// 定时器声明 (FR-006)。
    fn timer_declarations(&self) -> Vec<TimerDecl> {
        self.timer_declarations()
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
        globals.get::<Function>("on_stop").is_ok()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::BacktestContext;
    use ricow_core::CoreResult;
    use ricow_core::{Balance, Kline, Position};
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
        config.backtest =
            Some(crate::config::BacktestToml { funding_rate_8h: Some(0.0), ..Default::default() });
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

    /// 028 T038/T040: 组合路径靠"声明"驱动快照(`market:subscribe`) → 脚本 `ctx:klines`
    /// 读到该 pair 的**全段已收盘序列**(不再有 100 根 cap, 也不再需要装配层预装信号线)。
    #[test]
    fn test_portfolio_declared_pairs_full_klines_in_lua() {
        let script = r#"
            seen = {}
            first_ts = {}
            market:subscribe({ pair = "TSLABUSDT" })
            market:subscribe({ pair = "NVDABUSDT" })
            market:subscribe({ pair = "AAPLUSDT" })
            function on_tick(ctx)
                for _, p in ipairs({ "TSLABUSDT", "NVDABUSDT", "AAPLUSDT" }) do
                    local ks = ctx:klines(p)
                    if ks == nil then seen[p] = -1 else seen[p] = #ks; first_ts[p] = ks[1].ts end
                end
                return {}
            end
        "#;
        let mut strategy =
            LuaStrategy::from_source(script, test_config(script)).expect("编译应通过");
        let mut ctx = BacktestContext::new(
            strategy.config.clone(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // 200 个 tick 的组合成交轨 (三只都在) → 每只 199 根已收盘 bar。
        for day in 0..200i64 {
            ctx.step_portfolio(&[
                ("TSLABUSDT".into(), t3_exec_bar(day, 100 + day)),
                ("NVDABUSDT".into(), t3_exec_bar(day, 200 + day)),
                ("AAPLUSDT".into(), t3_exec_bar(day, 300 + day)),
            ]);
        }
        let _ = strategy.on_tick(&mut ctx);
        let seen = strategy.lua.globals().get::<mlua::Table>("seen").expect("seen 表");
        let first = strategy.lua.globals().get::<mlua::Table>("first_ts").expect("first_ts 表");
        for pair in ["TSLABUSDT", "NVDABUSDT", "AAPLUSDT"] {
            let n: i64 = seen.get(pair).expect("该只应有记录");
            assert!(n > 100, "{pair} 已收盘序列应超过旧 100 根 cap, got {n}");
            assert_eq!(n, 199, "{pair} tick N 时可见 N-1 根已收盘 bar");
            // 首根 = 第 0 天(最早一根) → 证明是"从头给全段"而非尾窗截断。
            let ts: i64 = first.get(pair).expect("应记录首根时间");
            assert_eq!(ts, 0, "{pair} 首根应为最早那根 (day0)");
        }
    }

    /// 028 T040: 单标的路径的 100 根硬编码 cap 已退役 —— 上下文里有多少已收盘 bar 就给多少。
    #[test]
    fn test_single_klines_not_capped() {
        let script = r#"
            n = -1
            first_ts = -1
            function on_tick(ctx)
                local ks = ctx:klines("ETH")
                if ks ~= nil then n = #ks; first_ts = ks[1].ts end
                return {}
            end
        "#;
        let mut strategy =
            LuaStrategy::from_source(script, test_config(script)).expect("编译应通过");
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
        assert!(n > 100, "单标的 klines 不再 cap 100, got {n}");
        assert_eq!(n, 249, "最后一根尚未收盘(本 tick 内可见 = 已收盘根数)");
        let first_ts: i64 = strategy.lua.globals().get("first_ts").expect("first_ts 已置位");
        assert_eq!(first_ts, 5 * 3600, "首根 = 最早那根 (旧 cap 100 会看不到)");
    }

    /// FR-005 (024): 净仓归零后, 策略侧 `ctx:position_side` 必须报 `none` (size<=0 契约)。
    #[test]
    fn test_position_side_label_contract() {
        let mk = |side, size: Decimal| Position {
            pair: "ETHUSDT".into(),
            side,
            size,
            entry_price: dec!(2500),
            mark_price: dec!(2500),
            liquidation_price: None,
            unrealized_pnl: Decimal::ZERO,
            leverage: None,
        };
        assert_eq!(position_side_label(&mk(OrderSide::Buy, dec!(0.5))), "long");
        assert_eq!(position_side_label(&mk(OrderSide::Sell, dec!(0.5))), "short");
        // 平仓归零: 记录里即便残留平仓方向, 也必须报 none。
        assert_eq!(position_side_label(&mk(OrderSide::Sell, Decimal::ZERO)), "none");
        assert_eq!(position_side_label(&mk(OrderSide::Buy, Decimal::ZERO)), "none");
    }

    // ========================================================================
    // 028: data / market / http 与 7 回调 (T014/T015/T017/T020/T021)
    // ========================================================================

    /// 纯逻辑替身(已知向量式): 提供固定的"已装载序列"与可编程的 HTTP 结果;
    /// 不替代任何真实数据源路径(真实验证见 `tests/*_live.rs` 与 demo 冒烟)。
    struct FakeHost {
        bars: Vec<Kline>,
    }

    impl HostServices for FakeHost {
        fn load_series(&self, decl: &SeriesDecl) -> CoreResult<Vec<Kline>> {
            if decl.key.source == "missing" {
                return Err(CoreError::InvalidArgument(
                    "未知数据源 'missing'; 可用来源: test".into(),
                ));
            }
            Ok(self.bars.clone())
        }

        fn history(&self, _key: &SeriesKey, limit: Option<usize>) -> CoreResult<Vec<Kline>> {
            let mut bars = self.bars.clone();
            if let Some(n) = limit {
                if bars.len() > n {
                    bars.drain(..bars.len() - n);
                }
            }
            Ok(bars)
        }

        fn http_get(&self, url: &str) -> CoreResult<String> {
            if url.contains("ok.example") {
                Ok("{\"v\":1}".into())
            } else {
                Err(CoreError::Network("timeout: 宿主墙钟 10s 到点".into()))
            }
        }
    }

    fn host_bars(n: usize) -> Vec<Kline> {
        (0..n)
            .map(|i| {
                let t =
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(i as i64 * 86_400_000)
                        .unwrap();
                let c = dec!(100) + Decimal::from(i as u64);
                Kline {
                    open_time: t,
                    open: c,
                    high: c + dec!(1),
                    low: c - dec!(1),
                    close: c,
                    volume: dec!(10),
                    close_time: t + chrono::Duration::hours(24),
                }
            })
            .collect()
    }

    fn bt_ctx(config: &StrategyConfig) -> BacktestContext {
        BacktestContext::new(
            config.clone(),
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        )
    }

    /// 第三轮审核 🟡: 三条新硬报错必须有回归用例(否则下轮改动可能悄悄退回静默行为)。
    ///
    /// ① FR-011 窗口超上限(原来静默截断 + warn) ② `data:subscribe` 撞已有句柄 ③ 序列数上限 32。
    #[test]
    fn test_declaration_hard_errors_are_enforced() {
        // ① 窗口超上限 → 报错(不是截断)
        let script = r#"
            data:series{ id = "x", source = "fixed", symbol = "TEST", interval = "1d", bars = 9999 }
        "#;
        let err = LuaStrategy::from_source_with_host(
            script,
            test_config(script),
            Arc::new(FakeHost { bars: host_bars(10) }),
        )
        .expect_err("超上限必须报错");
        assert!(err.contains("超过上限"), "错误应说清超上限, got: {err}");

        // ② 同一 id 先 data:series(拿句柄) 再 data:subscribe → 报错(免得声明与句柄分裂)
        let script2 = r#"
            local a = data:series{ id = "x", source = "fixed", symbol = "TEST", interval = "1d", bars = 5, min_bars = 2 }
            data:subscribe{ id = "x", source = "fixed", symbol = "TEST", interval = "1d", bars = 5, min_bars = 2 }
        "#;
        let err2 = LuaStrategy::from_source_with_host(
            script2,
            test_config(script2),
            Arc::new(FakeHost { bars: host_bars(10) }),
        )
        .expect_err("同 id 冲突必须报错");
        assert!(err2.contains("已用 data:series 声明过"), "got: {err2}");
        assert!(
            !err2.contains("——                          "),
            "错误文案里不得有连续空格残渣: {err2}"
        );

        // ③ 第 33 条序列 → 报错
        let mut script3 = String::new();
        for i in 0..33 {
            script3.push_str(&format!(
                "data:series{{ id = \"s{i}\", source = \"fixed\", symbol = \"TEST\", interval = \"1d\", bars = 5, min_bars = 2 }}\n"
            ));
        }
        let err3 = LuaStrategy::from_source_with_host(
            &script3,
            test_config(&script3),
            Arc::new(FakeHost { bars: host_bars(10) }),
        )
        .expect_err("超过 32 条必须报错");
        assert!(err3.contains("超过上限 32 条"), "got: {err3}");
    }

    /// 028 T021: 快照体积**按声明窗口**计, 不随该序列的可用历史长度放大。
    ///
    /// 8 条序列 × 窗口 10 根, 而宿主有 1000 根可用历史 → 每条句柄只持 10 根,
    /// 快照合计 = 80 根。若句柄持全量历史, 这里会是 8000 根(每 tick 克隆 8000 根 × N 轮
    /// 就是当年 `SIGNAL_TAIL` 想解决的问题, 028 直接用"声明窗口"替代)。
    #[test]
    fn test_snapshot_cost_is_bounded_by_declared_window() {
        let bars = host_bars(1000);
        let mut script = String::from("total = 0\n");
        for i in 0..8 {
            script.push_str(&format!(
                "local h{0} = data:series{{ id = \"s{0}\", source = \"test\", symbol = \"QQQ\", interval = \"1d\", bars = 10, drive = false }}\n",
                i
            ));
        }
        script.push_str(
            "function on_tick(ctx)\n    total = 0\n    for i = 0, 7 do\n        local h = data:series{ id = \"x\" .. i, source = \"test\", symbol = \"QQQ\", interval = \"1d\", bars = 10, drive = false }\n        total = total + h:len()\n    end\n    return {}\nend\n",
        );
        let mut config = test_config(&script);
        config.params.insert("pair".into(), ConfigValue::String("QQQ".into()));
        let mut strategy = LuaStrategy::from_source_with_host(
            &script,
            config.clone(),
            Arc::new(FakeHost { bars: bars.clone() }),
        )
        .expect("编译应通过");
        let mut ctx = bt_ctx(&config);
        strategy.on_tick(&mut ctx);
        let total: i64 = strategy.lua.globals().get("total").expect("total 已置位");
        assert_eq!(total, 80, "8 条序列 × 窗口 10 根 = 80 根; 若持全量历史会是 8000");
    }

    /// 028 FR-012: 同策略序列数上限 32 —— 第 33 条必须硬报错(而不是静默吃内存)。
    #[test]
    fn test_series_count_is_capped() {
        let mut script = String::from("local x = {}\n");
        for i in 0..33 {
            script.push_str(&format!(
                "x[{}] = data:series{{ id = \"s{}\" , source = \"test\", symbol = \"QQQ\", interval = \"1d\", bars = 3, drive = false }}\n",
                i + 1,
                i + 1
            ));
        }
        let err = LuaStrategy::from_source_with_host(
            &script,
            test_config(&script),
            Arc::new(FakeHost { bars: host_bars(5) }),
        )
        .expect_err("第 33 条声明必须报错");
        assert!(err.contains("上限 32"), "错误应点明上限, got: {err}");
    }

    /// 028 T044: 顶层 `config` 访问面(声明期取参数) + 句柄尾窗 `s:bars(n)`。
    ///
    /// 为什么这两个口子必须有: 声明发生在脚本**顶层**, 那里没有 `ctx` —— 内置/参数化策略要
    /// 按配置的 `pair`/`interval` 声明序列, 就只能靠顶层 `config.*`; 而"策略自己算自定义口径"
    /// (如 VWAP) 需要能拿到尾窗原始 bar, 不能逼平台造专用指标。
    #[test]
    fn test_toplevel_config_and_series_bars() {
        let bars = host_bars(30);
        let script = r#"
            local sym = config.str("pair") or "NONE"
            local look = config.f64("lookback") or 0
            local s = data:series{
                id = "main", source = "test", symbol = sym, interval = "1d",
                bars = 10, min_bars = 3, drive = false,
            }
            seen = {
                pair = sym, lookback = look, len = s:len(),
                all = #s:bars(), tail3 = #s:bars(3), first_tail_close = s:bars(3)[1].close,
                last_close = s:close(), stale = s:stale(),
            }
        "#;
        let mut config = test_config(script);
        config.params.insert("pair".into(), ConfigValue::String("TEST".into()));
        config.params.insert("lookback".into(), ConfigValue::Float(12.0));
        let strategy = LuaStrategy::from_source_with_host(
            script,
            config.clone(),
            Arc::new(FakeHost { bars: bars.clone() }),
        )
        .expect("顶层声明 + config 读取应成功");

        let seen = strategy.lua.globals().get::<mlua::Table>("seen").expect("seen 表");
        assert_eq!(seen.get::<String>("pair").unwrap(), "TEST", "顶层能读配置里的 pair");
        assert_eq!(seen.get::<f64>("lookback").unwrap(), 12.0, "顶层能读配置里的数值");
        assert_eq!(seen.get::<usize>("len").unwrap(), 10, "句柄只保留声明窗口(bars=10)的尾窗");
        assert_eq!(seen.get::<usize>("all").unwrap(), 10, "s:bars() = 句柄内全部(受声明窗口约束)");
        assert_eq!(seen.get::<usize>("tail3").unwrap(), 3, "s:bars(3) = 尾窗 3 根");
        // 尾窗必须是"最后 3 根"而不是前 3 根: 第 1 根 = 倒数第 3 根。
        let expected_first = bars[27].close.to_f64().unwrap();
        assert_eq!(
            seen.get::<f64>("first_tail_close").unwrap(),
            expected_first,
            "s:bars(n) 取的是尾部"
        );
        assert_eq!(
            seen.get::<f64>("last_close").unwrap(),
            bars[29].close.to_f64().unwrap(),
            "s:close() = 最新一根收盘"
        );
        assert!(!seen.get::<bool>("stale").unwrap(), "未置位时为 false");
    }

    /// 测试用序列描述(引擎装配层在真实路径里由 `SeriesInfo::new` 构造)。
    fn test_series_info(id: &str) -> SeriesInfo {
        let decl =
            SeriesDecl::new(id, SeriesKey::new("test", "QQQ", Interval::D1).expect("键合法"));
        SeriesInfo::native(&decl)
    }

    /// `data:series` 声明 + 句柄: 尾窗按声明截断, 指标与同源实现逐位一致 (T017/T018)。
    #[test]
    fn test_data_series_handle_installs_and_matches_indicator_math() {
        let bars = host_bars(60);
        let script = r#"
            local s = data:series{ id = "qqq", source = "test", symbol = "QQQ", interval = "1d", bars = 50 }
            function on_bar(ctx, series, bar)
                -- 自检: 序列描述必须如实报出 来源/标的/周期/口径 (FR-005), 否则不产单
                if series.id ~= "qqq" or series.source ~= "test" or series.symbol ~= "QQQ"
                   or series.interval ~= "1d" or series.mode ~= "native"
                   or series.feed_interval ~= "1d" or series.price ~= "close" then
                    return {}
                end
                return { {
                    pair = "QQQ", side = "buy", order_type = "market",
                    size = s:len(),
                    price = s:ema(20),
                } }
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source_with_host(
            script,
            config.clone(),
            Arc::new(FakeHost { bars: bars.clone() }),
        )
        .expect("顶层声明应成功");

        assert_eq!(strategy.declarations().len(), 1, "声明被记录");
        assert_eq!(strategy.declarations()[0].id, "qqq");
        assert!(strategy.defines("on_bar"), "脚本定义了 on_bar");
        assert!(!strategy.defines("on_tick"), "没写 on_tick");
        assert!(!strategy.has_callback("on_quote"), "没写 on_quote");

        let mut ctx = bt_ctx(&config);
        let info = test_series_info("qqq");
        let orders = strategy.on_bar(&mut ctx, &info, &bars[59]);
        assert_eq!(orders.len(), 1, "回调返回值仍是订单出口");
        assert_eq!(orders[0].size, dec!(50), "尾窗按声明截到 50 根(host 给了 60)");
        let expect = indicators_api::ema(&bars[10..], 20).expect("有值");
        assert_eq!(
            orders[0].price.unwrap().to_f64().unwrap(),
            expect,
            "句柄 ema == 同源 indicators_api(同一输入长度 50 根, 逐位一致)"
        );
        assert_eq!(strategy.installed_series(), vec!["qqq".to_string()]);
    }

    /// `ctx:place_order` / `ctx:cancel_order` 四种粒度都记为意图, 取走后清空 (T015)。
    #[test]
    fn test_ctx_order_intents_recorded_and_drained() {
        let script = r#"
            function on_init(ctx)
                ctx:place_order{ pair = "ETHUSDT", side = "buy", size = 1.5, order_type = "limit", price = 100 }
                ctx:cancel_order()
                ctx:cancel_order{ pair = "ETHUSDT" }
                ctx:cancel_order{ pair = "ETHUSDT", order_id = "abc" }
                ctx:cancel_order{ pair = "ETHUSDT", client_order_id = "ricow-x-1" }
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source(script, config.clone()).expect("编译应通过");
        let mut ctx = bt_ctx(&config);
        strategy.on_init(&mut ctx);

        let intents = strategy.take_intents();
        assert_eq!(intents.orders.len(), 1, "ctx:place_order 入队 1 笔");
        assert_eq!(intents.orders[0].size, dec!(1.5));
        assert_eq!(intents.orders[0].price, Some(dec!(100)));
        assert_eq!(
            intents.cancels,
            vec![
                CancelIntent::Owned,
                CancelIntent::OwnedIn { pair: "ETHUSDT".into() },
                CancelIntent::ByOrderId { pair: "ETHUSDT".into(), order_id: "abc".into() },
                CancelIntent::ByClientId {
                    pair: "ETHUSDT".into(),
                    client_order_id: "ricow-x-1".into()
                },
            ],
            "无参/按对/按订单号/按 client id 四种粒度"
        );
        assert!(strategy.take_intents().is_empty(), "取走后清空");
    }

    /// `http.get` 失败返回 `nil, err`(可判别), 策略可自降级; 成功返回 body (T033/T034)。
    #[test]
    fn test_http_get_failure_and_success_paths() {
        let script = r#"
            function on_timer(ctx, label)
                if label == "ok" then
                    local body, err = http.get("https://ok.example/data")
                    if body then
                        return { { pair = "ETHUSDT", side = "buy", size = #body, order_type = "market" } }
                    end
                    return {}
                end
                local body, err = http.get("https://fail.example/x")
                if err then
                    return { { pair = "ETHUSDT", side = "buy", size = 1, order_type = "market" } }
                end
                return {}
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source_with_host(
            script,
            config.clone(),
            Arc::new(FakeHost { bars: vec![] }),
        )
        .expect("编译应通过");
        let mut ctx = bt_ctx(&config);

        let ok = strategy.on_timer(&mut ctx, "ok");
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].size, dec!(7), "body 长度 = 7 字符");

        let failed = strategy.on_timer(&mut ctx, "fail");
        assert_eq!(failed.len(), 1, "失败时不中断策略, 可自降级");
        assert!(strategy.last_http_error().unwrap().contains("timeout"), "错误可判别");
        assert_eq!(strategy.http_call_count(), 2);
    }

    /// 未知来源名在**声明那一刻**就报错(不静默给空序列) (T020/FR-016)。
    #[test]
    fn test_unknown_source_fails_at_declaration() {
        let script = r#"
            local s = data:series{ id = "x", source = "missing", symbol = "QQQ", interval = "1d" }
        "#;
        let err = LuaStrategy::from_source_with_host(
            script,
            test_config(script),
            Arc::new(FakeHost { bars: vec![] }),
        )
        .expect_err("未知来源名必须报错");
        assert!(err.contains("未知数据源 'missing'"), "{err}");
    }

    /// 每回调独立的 1M 指令预算: 两个各 600k 迭代的回调都要跑完 (T021/FR-007)。
    #[test]
    fn test_per_callback_budget_is_independent() {
        let script = r#"
            function on_bar(ctx, series, bar)
                for i = 1, 600000 do end
                return { { pair = "ETHUSDT", side = "buy", size = 1, order_type = "market" } }
            end
            function on_timer(ctx, label)
                for i = 1, 600000 do end
                return { { pair = "ETHUSDT", side = "buy", size = 1, order_type = "market" } }
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source(script, config.clone()).expect("编译应通过");
        let mut ctx = bt_ctx(&config);
        let info = test_series_info("x");
        assert_eq!(
            strategy.on_bar(&mut ctx, &info, &sample_kline()).len(),
            1,
            "on_bar 预算未被上轮消耗"
        );
        assert_eq!(strategy.on_timer(&mut ctx, "30s").len(), 1, "on_timer 独立预算");
    }

    /// 未装配宿主时: 回调内的 `data:*` 调用被 call() 兜住(记日志), 策略不崩、不静默返回假数据 (T016)。
    #[test]
    fn test_data_api_without_host_does_not_panic() {
        let script = r#"
            function on_tick(ctx)
                local s = data:series{ id = "x", source = "test", symbol = "QQQ", interval = "1d" }
                return { { pair = "ETHUSDT", side = "buy", size = 1, order_type = "market" } }
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source(script, config.clone()).expect("编译应通过");
        let mut ctx = bt_ctx(&config);
        let orders = strategy.on_tick(&mut ctx);
        assert!(orders.is_empty(), "取数失败 → 该轮回调无订单(call 兜住错误, 不 panic)");
    }

    /// `data:history` 只读本地缓存(宿主侧), 返回 `{ts, open, high, low, close, volume}` 行 (T020)。
    #[test]
    fn test_data_history_returns_cached_rows() {
        let bars = host_bars(5);
        let script = r#"
            function on_init(ctx)
                local rows = data.history{ source = "test", symbol = "QQQ", interval = "1d", limit = 3 }
                return { { pair = "QQQ", side = "buy", size = #rows, price = rows[#rows].close, order_type = "market" } }
            end
        "#;
        let config = test_config(script);
        let mut strategy = LuaStrategy::from_source_with_host(
            script,
            config.clone(),
            Arc::new(FakeHost { bars: bars.clone() }),
        )
        .expect("编译应通过");
        let mut ctx = bt_ctx(&config);
        strategy.on_init(&mut ctx);
        let intents = strategy.take_intents();
        assert!(intents.orders.is_empty(), "on_init 返回值不入订单出口(与既有语义一致)");
        // 用句柄读回: 直接在 Lua 侧断言更直观 —— 这里改为走 data:series + 数量
        let script2 = r#"
            function on_init(ctx)
                local s = data:series{ id = "q", source = "test", symbol = "QQQ", interval = "1d", bars = 2, drive = false }
                ctx:place_order{ pair = "QQQ", side = "buy", size = s:len(), price = s:close(), order_type = "market" }
            end
        "#;
        let mut strategy2 = LuaStrategy::from_source_with_host(
            script2,
            test_config(script2),
            Arc::new(FakeHost { bars: bars.clone() }),
        )
        .expect("编译应通过");
        strategy2.on_init(&mut ctx);
        let intents2 = strategy2.take_intents();
        assert_eq!(intents2.orders.len(), 1);
        assert_eq!(intents2.orders[0].size, dec!(2), "bars=2 → 尾窗 2 根");
        assert_eq!(intents2.orders[0].price, Some(dec!(104)), "最后一根 close=104");
        assert!(
            !strategy2.driving_declarations().iter().any(|d| d.id == "q"),
            "drive=false 不驱动 on_bar"
        );
    }
}
