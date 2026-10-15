//! 054 P4: 样本外检验 / 分段验证 / 滚动验证(**异步作业**, 复用回测作业队列)。
//!
//! 存在理由(审计 #3 / #6): 用户跑完一次回测看到的数字, 全部来自**同一段**历史。
//! 参数寻优又是在这同一段上挑出来的最好一组 —— 于是"回测很漂亮"这件事几乎不携带
//! 关于未来的信息。本模块把"同一段里挑出来的参数, 在没见过的数据上是什么样"做成
//! **界面上点一下就能看**的东西, 而不是要求用户自己去理解 walk-forward 是什么。
//!
//! ## 三条硬纪律(改本文件前先看)
//!
//! 1. **禁止随机 / 交错切分**(D4): 只能按时间顺序切。随机切会把未来信息漏进"训练段",
//!    自己制造前视偏差 —— 那比不做检验更糟, 因为它会给出一个**看起来很好的**数字
//!    (见 `specs/backtest.md §一`, 那是本项目的最高约束)。
//! 2. **术语纪律**(T32): 「分段验证」与「滚动验证(含重新调参)」是**两件事**,
//!    界面上不许统称 walk-forward —— 统称会让人以为两者可以互相比较。
//! 3. **退化只描述、不定性**(D6): 样本外变差就报"样本外退化"并给建议,
//!    **绝不说"过拟合"** —— 成因有参数过拟合、行情切换、数据口径变化等多种,
//!    一概归因超出证据范围。
//!
//! ## 门槛分层(D5)的口径
//!
//! `< 30 天` = 样本不足(**不出结论**, 只引导拉长窗口);
//! `≥ 30 天` = 已达最低可信线; `≥ 90 天` = 样本充足。
//! 30/90 是**两条线**, 不是三个并列档位 —— 90 是更高的自选标准, 不是"及格线"。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Json, Json as AxumJson};
use chrono::{TimeZone, Utc};
use ricow_strategy::ConfigValue;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::backtest_jobs::{guarded, parse_interval, redact_root, StartedReply};
use super::strategy_io::config_values;
use super::{WebError, WebState};
use crate::commands::backtest::{run_backtest_full, BacktestOutcome, BacktestRunSpec};

// ---- 常量(门槛与切分比例的**单点定义**) ----

/// 样本外占比: 后 30%。**固定按时间顺序取尾部**(D4)。
const OUT_OF_SAMPLE_RATIO: f64 = 0.30;

/// 最低可信线(天): 样本外少于它就不给结论。
const MIN_OOS_DAYS: u32 = 30;

/// 样本充足线(天): 更高的自选标准, 不是及格线。
const AMPLE_OOS_DAYS: u32 = 90;

/// 形态 A 的分段下限(天): 每段少于它则段内没有统计意义。
const MIN_SEGMENT_DAYS: u32 = 30;

/// 形态 A 的段数上下限。
const MIN_SEGMENTS: u32 = 2;
const MAX_SEGMENTS: u32 = 5;

/// T32: 每段曲线下发前的抽稀上限(点)。5 段 × 150 = 750 点, 足够画出形状又不撑爆响应体。
const CURVE_MAX_POINTS: usize = 150;

/// 形态 B 的段数上限: 每轮都要重新寻优, 成本高两个数量级 —— 压到 3 段是明码标价的一部分。
const MAX_WALK_B_SEGMENTS: u32 = 3;

/// 形态 A 的分段基准: 每 60 天一段(与 MIN_SEGMENT_DAYS 同量级, 保证每段都有统计意义)。
const SEGMENT_DAYS_UNIT: u32 = 60;

// ---- 纯函数(全部可单测) ----

/// 一个日期区间(毫秒时间戳; `end` 不含)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Span {
    pub(super) start_ms: i64,
    pub(super) end_ms: i64,
}

impl Span {
    fn days(&self) -> f64 {
        ((self.end_ms - self.start_ms) as f64) / 86_400_000.0
    }
}

/// D4: 按时间顺序切前 70% / 后 30%(**纯函数**)。
///
/// 只按时间切 —— 随机抽样会把未来信息漏进前段, 自己制造前视偏差。
pub(super) fn split_window(total: Span) -> (Span, Span) {
    let cut = total.start_ms
        + ((total.end_ms - total.start_ms) as f64 * (1.0 - OUT_OF_SAMPLE_RATIO)) as i64;
    (Span { start_ms: total.start_ms, end_ms: cut }, Span { start_ms: cut, end_ms: total.end_ms })
}

/// D5: 样本外天数的门槛分层(纯函数)。
///
/// 返回 `"insufficient" / "minimum" / "ample"` —— 三档是**两条线**切出来的,
/// 界面上要如实写成"低于最低可信线 / 已达最低可信线 / 样本充足", 不能并列成三个选项。
pub(super) fn oos_threshold(oos_days: f64) -> &'static str {
    if oos_days < MIN_OOS_DAYS as f64 {
        "insufficient"
    } else if oos_days < AMPLE_OOS_DAYS as f64 {
        "minimum"
    } else {
        "ample"
    }
}

/// D6: 是否打「样本外退化」标(纯函数)。
///
/// 判据: 样本外净盈亏 < 0 **或** 样本外回撤 > 样本内 × 1.5。
/// `None` = 缺数据算不出来 —— 算不出就**不打标**(不打标 ≠ 没退化, 界面要照实说)。
///
/// `in_net` 缺失时同样返回 `None`: 退化是「样本外**相对样本内**变差」, 样本内那一段
/// 没跑出来就无从比较 —— 这时拿样本外的绝对值去判断等于凭空立一个标准。
pub(super) fn is_degraded(
    in_net: Option<f64>,
    out_net: Option<f64>,
    in_dd: Option<f64>,
    out_dd: Option<f64>,
) -> Option<bool> {
    in_net?;
    let out_net = out_net?;
    let mut bad = out_net < 0.0;
    if let (Some(i), Some(o)) = (in_dd, out_dd) {
        if i > 0.0 && o > i * 1.5 {
            bad = true;
        }
    }
    Some(bad)
}

/// 形态 A: 窗口均分 N 段(纯函数)。段数 = `clamp(floor(天数/60), 2, 5)`,
/// 且**每段 ≥ 30 天** —— 否则报样本不足(不给一堆没有统计意义的段)。
pub(super) fn walk_segments(total: Span) -> Vec<Span> {
    let days = total.days();
    let n = ((days / SEGMENT_DAYS_UNIT as f64).floor() as u32).clamp(MIN_SEGMENTS, MAX_SEGMENTS);
    if n < MIN_SEGMENTS {
        return Vec::new();
    }
    let per = ((total.end_ms - total.start_ms) as f64 / n as f64) as i64;
    if per <= 0 || (per as f64) / 86_400_000.0 < MIN_SEGMENT_DAYS as f64 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let start = total.start_ms + per * i as i64;
        // 最后一段吃掉取整余数, 免得尾部几天被丢掉(那会让"最后一段更短"变成假象)。
        let end = if i + 1 == n { total.end_ms } else { start + per };
        out.push(Span { start_ms: start, end_ms: end });
    }
    out
}

/// 形态 B 的段数上限(纯函数): 与形态 A 同算法, 但压到 3 段。
pub(super) fn walk_b_segments(total: Span) -> Vec<Span> {
    let mut v = walk_segments(total);
    if v.len() > MAX_WALK_B_SEGMENTS as usize {
        v.truncate(MAX_WALK_B_SEGMENTS as usize);
    }
    v
}

fn ymd(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

// ---- HTTP ----

/// `POST /api/backtest/validate` 请求体。
#[derive(Debug, Deserialize)]
pub(super) struct ValidateRequest {
    strategy: String,
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    interval: Option<String>,
    /// 回测天数(缺省 90)。
    #[serde(default)]
    days: Option<u32>,
    /// 模式: `split`(样本内外切分) | `walk`(分段/滚动验证)。
    #[serde(default)]
    mode: Option<String>,
    /// 形态: `A`(固定参数分段, 默认) | `B`(每段前重新寻优, 仅 walk 模式)。
    #[serde(default)]
    form: Option<String>,
    /// 形态 B 的**参数候选网格**: `[{key, values:[...]}]`。
    ///
    /// 必填(形态 B 下): "重新调参"必须知道在哪些参数上、按哪些候选去挑 —— 后端猜不出来,
    /// 而猜一个默认网格会让"最优参数"变成平台的产物, 用户根本不知道它被调过什么。
    #[serde(default)]
    dims: Option<Vec<OptimizeDim>>,
    /// 形态 B 的排序指标: `net_pnl`(默认) | `sharpe` | `annual_return`。
    #[serde(default)]
    metric: Option<String>,
}

/// 形态 B 的一维候选。
#[derive(Debug, Clone, Deserialize)]
pub(super) struct OptimizeDim {
    key: String,
    values: Vec<Value>,
}

/// 一段的结果(前端按段渲染一组指标卡)。
#[derive(Debug, Serialize)]
pub(super) struct SegmentOut {
    /// 段序号(从 1 起)。
    index: usize,
    /// 段名(前端直接用; 服务端已经替用户把日期拼好)。
    label: String,
    start: String,
    /// 不含。
    end: String,
    days: u32,
    /// 该段指标(与普通回测同一套键)。
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<Value>,
    /// T32: 该段的**权益曲线**(已抽稀, `{t:[毫秒], eq:[金额]}`)。
    ///
    /// 存在理由: 前端要把各段拼成一条曲线 —— 形态 A 的各段首尾相接, 拼出来就是全窗口曲线;
    /// 形态 B 拼出来的是**只在各验证段上**的收益, 也就是样本外曲线。两条线含义不同,
    /// 所以必须分别可画, 不能只给每段一个汇总数字(那样"曲线形状"信息全丢了)。
    ///
    /// 抽稀到 [`CURVE_MAX_POINTS`] 点: 一次验证最多 5 段, 全分辨率下发对超长窗口太大。
    #[serde(skip_serializing_if = "Option::is_none")]
    curve: Option<Value>,
    /// 该段是否**未跑出来**(如 K 线不足); 跑不出来就如实留空, 不给 0。
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// 验证结果(整个 `validate` 字段的形状)。
#[derive(Debug, Serialize)]
pub(super) struct ValidateOut {
    /// `split` | `walk`。
    mode: String,
    /// walk 模式的形态: `A` | `B`(split 模式不出现)。
    #[serde(skip_serializing_if = "Option::is_none")]
    form: Option<String>,
    interval: String,
    /// 总窗口(前端原样显示, 免得另算一份)。
    window: (String, String),
    days: u32,
    segments: Vec<SegmentOut>,
    /// 样本外段落的序号(1 起; split 模式恒为 2)。
    #[serde(skip_serializing_if = "Option::is_none")]
    oos_index: Option<usize>,
    /// 样本外天数。
    #[serde(skip_serializing_if = "Option::is_none")]
    oos_days: Option<u32>,
    /// 门槛分层: `insufficient` / `minimum` / `ample`。
    #[serde(skip_serializing_if = "Option::is_none")]
    threshold: Option<&'static str>,
    /// D6: 是否退化(`None` = 数据不足算不出来)。
    degraded: Option<bool>,
    /// 一句话结论(服务端写好的中文, 前端**原样展示**, 不自己拼)。
    note: String,
}

fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

/// T32: 把一段的权益曲线抽稀成 `{t, eq}`(纯函数; 空序列 → `None`)。
///
/// 抽稀用**整步**而不是重采样: 曲线是路径相关的量, 任何插值都会造出没发生过的形状。
fn downsample_curve(times: &[i64], equity: &[f64]) -> Option<Value> {
    if times.is_empty() || equity.len() != times.len() {
        return None;
    }
    let stride = times.len().div_ceil(CURVE_MAX_POINTS).max(1);
    let mut t: Vec<i64> = Vec::new();
    let mut eq: Vec<f64> = Vec::new();
    for (i, ms) in times.iter().enumerate() {
        if i % stride == 0 {
            t.push(*ms);
            eq.push(equity[i]);
        }
    }
    // 末点强制保留: 段的终值是最有意义的那个点(决定下一段从哪儿接)。
    if t.last() != times.last() {
        t.push(times[times.len() - 1]);
        eq.push(equity[equity.len() - 1]);
    }
    Some(json!({ "t": t, "eq": eq }))
}

/// 把窗口切成 Spec 的两段并各跑一次(共用 `run_backtest_full`, 与页面回测同一内核)。
///
/// 返回 `(指标, 曲线)`: 两者同源(同一次回测), 分开是为了让"某段没跑出来"能只留空指标
/// 而不编造曲线。
async fn run_span(
    base: &BacktestRunSpec,
    root: Arc<std::path::PathBuf>,
    span: Span,
) -> Result<(Value, Option<Value>), String> {
    let spec = BacktestRunSpec {
        root: (*root).clone(),
        start: Some(ymd(span.start_ms)),
        end: Some(ymd(span.end_ms)),
        ..base.clone()
    };
    match run_backtest_full(spec).await {
        Ok(out) => {
            let c = out.chart();
            let curve = downsample_curve(&c.times, &c.equity);
            Ok((serde_json::to_value(out.metrics()).unwrap_or(Value::Null), curve))
        }
        Err(e) => Err(redact_root(&e.to_string(), &root)),
    }
}

/// `POST /api/backtest/validate`: 发起一次验证作业(202 + job_id)。
///
/// 与 `/api/backtest` 共用作业队列与 409 busy: 一次验证要跑 N 段回测(形态 B 还要寻优),
/// 不允许和同一策略的另一次回测并行 —— 否则 K 线请求会互相挤, 两边都变慢。
pub(super) async fn start_validate(
    State(state): State<WebState>,
    payload: Result<AxumJson<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<StartedReply>), WebError> {
    let Json(body) =
        payload.map_err(|e| WebError::bad_request(format!("请求体不是合法 JSON: {e}")))?;
    let req: ValidateRequest = serde_json::from_value(body).map_err(|e| {
        WebError::bad_request(format!(
            "请求体字段非法: 需要 strategy 字符串; pair/market/interval/mode/form 可选字符串, \
             days 整数: {e}"
        ))
    })?;

    let strategy = req.strategy.trim();
    if strategy.is_empty() {
        return Err(WebError::bad_request("strategy(策略名)不能为空"));
    }
    ricow_strategy::validate_strategy_name(strategy)
        .map_err(|e| WebError::bad_request_code(e, "invalid_name"))?;
    super::backtest_jobs::reject_unavailable(strategy)?;
    let known = crate::strategies::catalog::is_builtin_id(strategy)
        || state.root.join("strategies").join(format!("{strategy}.toml")).is_file();
    if !known {
        return Err(WebError::not_found(format!(
            "没有策略 {strategy}: 验证只接受内置策略或已保存的用户策略"
        )));
    }
    let interval = parse_interval(req.interval.as_deref()).map_err(WebError::bad_request)?;
    let days = req.days.unwrap_or(90);
    if !(1..=3650).contains(&days) {
        return Err(WebError::bad_request(format!("days 必须是 1..=3650 的整数, 收到: {days}")));
    }
    let mode = req.mode.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("split");
    if mode != "split" && mode != "walk" {
        return Err(WebError::bad_request(format!(
            "mode 只能是 split(样本内外切分) 或 walk(分段/滚动验证), 收到: {mode}"
        )));
    }
    let form = match req.form.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => "A".to_string(),
        Some(f) if f.eq_ignore_ascii_case("a") => "A".to_string(),
        Some(f) if f.eq_ignore_ascii_case("b") => "B".to_string(),
        Some(f) => {
            return Err(WebError::bad_request(format!(
                "form 只能是 A(固定参数分段) 或 B(每段前重新寻优), 收到: {f}"
            )))
        }
    };
    // 形态 B 只跟 walk 搭配: 它做的事就是"每段前重新调参", 套到 split 上没有意义。
    if mode == "split" && form == "B" {
        return Err(WebError::bad_request(
            "form=B 只对 mode=walk 有意义: 样本内外切分不重新调参, 它的参数就是你现在这组",
        ));
    }

    // 形态 B 的候选网格: 必填 —— "重新调参"必须知道在哪些参数上挑, 后端猜不出来,
    // 而猜一个默认网格会让"最优参数"变成平台的产物(用户根本不知道它被调过什么)。
    if mode == "walk" && form == "B" && req.dims.as_ref().map(|d| d.is_empty()).unwrap_or(true) {
        return Err(WebError::bad_request(
            "form=B(含重新调参)必须给 dims: [{key, values:[...]}], 例如 \
             [{\"key\":\"spacing_pct\",\"values\":[0.3,0.5,0.8]}] —— 没有候选就无法寻优",
        ));
    }
    let metric = match req.metric.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => "net_pnl".to_string(),
        Some(m) if m == "net_pnl" || m == "sharpe" || m == "annual_return" => m.to_string(),
        Some(m) => {
            return Err(WebError::bad_request(format!(
                "metric 只能是 net_pnl / sharpe / annual_return, 收到: {m}"
            )))
        }
    };

    let market = match req.market.as_deref() {
        Some(m) => Some(super::strategy_io::parse_market(m).map_err(WebError::bad_request)?),
        None => None,
    };

    let base = BacktestRunSpec {
        root: (*state.root).clone(),
        strategy: strategy.to_string(),
        pair: clean(req.pair),
        days,
        interval: interval.to_string(),
        start: None,
        end: None,
        market: market.map(str::to_string),
        position_mode: None,
        params: std::collections::HashMap::new(),
        script_path: None,
        raw_code: None,
        fee: None,
        fee_maker: None,
        fee_taker: None,
        slippage_bps: None,
        limit_fill_penetration_bps: None,
        cash: None,
        leverage: None,
        max_leverage: None,
        mmr_pct: None,
        funding_rate: None,
        klines_market: None,
        interval_explicit: req.interval.as_deref().map(str::trim).is_some_and(|s| !s.is_empty()),
        close_at_end: false,
        export_dir: None,
    };

    let job_id = state.jobs.start(strategy, Utc::now()).map_err(|_busy| {
        WebError::conflict(
            format!(
                "策略 {strategy} 已有回测/验证在运行: 同一策略同时只允许一个作业, \
                     请等当前作业结束后再发起(可轮询原 job_id 取结果)"
            ),
            "busy",
        )
    })?;

    let jobs = Arc::clone(&state.jobs);
    let id = job_id.clone();
    let strategy_name = strategy.to_string();
    let job_root = Arc::clone(&state.root);
    let mode_owned = mode.to_string();
    let dims_owned = req.dims.unwrap_or_default();
    tokio::spawn(async move {
        let result = guarded(async {
            run_validate(base, job_root, mode_owned, form, dims_owned, metric).await
        })
        .await;
        match result {
            Ok(Ok(v)) => jobs.finish_validate(&id, v, Utc::now()),
            Ok(Err(msg)) => jobs.finish_error(&id, msg, Utc::now()),
            Err(msg) => {
                tracing::error!(target: "web", strategy = %strategy_name, "验证内核 panic: {msg}");
                jobs.finish_error(&id, format!("验证内核异常退出 (panic): {msg}"), Utc::now());
            }
        }
    });

    Ok((StatusCode::ACCEPTED, Json(StartedReply { job_id })))
}

/// 验证主体(与作业登记解耦, 便于单测直接调)。
async fn run_validate(
    base: BacktestRunSpec,
    root: Arc<std::path::PathBuf>,
    mode: String,
    form: String,
    dims: Vec<OptimizeDim>,
    metric: String,
) -> Result<Value, String> {
    let now = Utc::now().timestamp_millis();
    let total = Span { start_ms: now - (base.days as i64) * 86_400_000, end_ms: now };
    // 形态 B 的段要留第 1 段当训练数据, 所以它的分段按"验证段数 ≥ 1"判定。
    let segs: Vec<Span> = if mode == "split" {
        let (a, b) = split_window(total);
        vec![a, b]
    } else if form == "B" {
        walk_b_segments(total)
    } else {
        walk_segments(total)
    };
    if segs.is_empty() {
        return Err(format!(
            "窗口 {} 天不够切: 分段验证要求每段至少 {} 天, 且至少 {} 段 —— 请把窗口拉到 {} 天以上",
            base.days,
            MIN_SEGMENT_DAYS,
            MIN_SEGMENTS,
            MIN_SEGMENT_DAYS * MIN_SEGMENTS
        ));
    }
    if mode == "walk" && form == "B" && segs.len() < 2 {
        return Err(format!(
            "滚动验证(含重新调参)要求至少切出 2 段: 第 1 段要留给训练, 之后才能验证 —— \
             当前窗口 {} 天不够, 请拉长窗口",
            base.days
        ));
    }
    let out_segs: Vec<SegmentOut> = if mode == "walk" && form == "B" {
        run_walk_b(&base, &root, total, &segs, &dims, &metric).await
    } else {
        let mut v: Vec<SegmentOut> = Vec::with_capacity(segs.len());
        for (i, s) in segs.iter().enumerate() {
            match run_span(&base, Arc::clone(&root), *s).await {
                Ok((m, curve)) => v.push(SegmentOut {
                    index: i + 1,
                    label: segment_label(&mode, &form, i, segs.len()),
                    start: ymd(s.start_ms),
                    end: ymd(s.end_ms),
                    days: s.days().round().max(1.0) as u32,
                    metrics: Some(m),
                    curve,
                    reason: None,
                }),
                Err(e) => v.push(SegmentOut {
                    index: i + 1,
                    label: segment_label(&mode, &form, i, segs.len()),
                    start: ymd(s.start_ms),
                    end: ymd(s.end_ms),
                    days: s.days().round().max(1.0) as u32,
                    metrics: None,
                    curve: None,
                    // 某段跑不出来就**如实留空**: 拿别的段顶上或者填 0 都是编造。
                    reason: Some(e),
                }),
            }
        }
        v
    };
    if out_segs.is_empty() {
        return Err(
            "没有跑出任何一段: 形态 B 需要可用的参数候选网格(每组至少一个合法取值)".to_string()
        );
    }

    let mut out = ValidateOut {
        mode: mode.clone(),
        form: if mode == "walk" { Some(form.clone()) } else { None },
        interval: base.interval.clone(),
        window: (ymd(total.start_ms), ymd(total.end_ms)),
        days: base.days,
        segments: out_segs,
        oos_index: None,
        oos_days: None,
        threshold: None,
        degraded: None,
        note: String::new(),
    };

    if mode == "split" {
        let oos = out.segments.get(1);
        let oos_days = oos.map(|s| s.days).unwrap_or(0);
        out.oos_index = Some(2);
        out.oos_days = Some(oos_days);
        let th = oos_threshold(oos_days as f64);
        out.threshold = Some(th);
        let (in_m, out_m) = (out.segments[0].metrics.clone(), out.segments[1].metrics.clone());
        let pick = |m: Option<&Value>, k: &str| m.and_then(|v| num(v, k));
        // 算不出(= None)就**不打标**, 绝不退化成"没退化"(D6)。
        out.degraded = is_degraded(
            pick(in_m.as_ref(), "net_pnl"),
            pick(out_m.as_ref(), "net_pnl"),
            pick(in_m.as_ref(), "max_drawdown_pct"),
            pick(out_m.as_ref(), "max_drawdown_pct"),
        );
        out.note = split_note(th, oos_days, out.degraded);
    } else if form == "B" {
        out.note = walk_b_note(out.segments.len(), combos_len(&dims));
    } else {
        out.note = walk_note(&form, out.segments.len());
    }

    serde_json::to_value(out).map_err(|e| format!("验证结果序列化失败: {e}"))
}

fn segment_label(mode: &str, form: &str, i: usize, n: usize) -> String {
    if mode == "split" {
        if i == 0 {
            "样本内(前 70%)".to_string()
        } else {
            "样本外(后 30%)".to_string()
        }
    } else {
        let kind = if form == "B" { "滚动" } else { "分段" };
        format!("{kind} {}/{}", i + 1, n)
    }
}

/// 切分结论(服务端写好的一句话, 前端原样展示)。
fn split_note(th: &str, oos_days: u32, degraded: Option<bool>) -> String {
    match th {
        "insufficient" => format!(
            "样本外只有 {oos_days} 天, 低于最低可信线 {MIN_OOS_DAYS} 天 —— 这段太短, 给不出可信结论, \
             也不该拿它下判断。把窗口拉长(样本外 ≥ {AMPLE_OOS_DAYS} 天更稳)再跑一次。"
        ),
        "minimum" => match degraded {
            Some(true) => format!(
                "样本外 {oos_days} 天, 刚过最低可信线 {MIN_OOS_DAYS} 天。样本外相对样本内**明显变差** —— \
                 这说明它在这段历史之外的表现没那么好, 成因可能有多种(参数挑得太贴、行情换了风格), \
                 这里**不下定性结论**。建议拉到样本外 ≥ {AMPLE_OOS_DAYS} 天再看一次。"
            ),
            Some(false) => format!(
                "样本外 {oos_days} 天, 刚过最低可信线 {MIN_OOS_DAYS} 天, 没有出现明显退化 —— \
                 但这段仍然偏短, 别把它当成充分证据。"
            ),
            None => format!(
                "样本外 {oos_days} 天, 刚过最低可信线 {MIN_OOS_DAYS} 天; 退化判定算不出来(缺指标), \
                 所以**没有**给出「没退化」的结论。"
            ),
        },
        _ => match degraded {
            Some(true) => format!(
                "样本外 {oos_days} 天(样本充足)。样本外相对样本内**明显变差** —— 这里只报「样本外退化」: \
                 成因可能有多种(参数挑得太贴这段历史、行情换了风格、成本口径变了), \
                 一概归因超出证据范围。建议换一段窗口再验证一次。"
            ),
            Some(false) => format!(
                "样本外 {oos_days} 天(样本充足), 没有出现明显退化。这仍然只是**一段**历史之外的表现, \
                 不代表未来收益。"
            ),
            None => format!(
                "样本外 {oos_days} 天(样本充足); 退化判定算不出来(缺指标), 所以**没有**给出「没退化」的结论。"
            ),
        },
    }
}

/// 把前端给的 `[{key, values:[...]}]` 翻译成寻优用的候选网格(纯函数)。
///
/// 某个维度一个合法取值都解析不出来就**整维丢弃**(不是报错): 用户可能勾选了
/// 一个当前策略并不认识的参数 —— 报"整个网格非法"会把能用的那几维也一起否掉。
fn candidate_grid(dims: &[OptimizeDim]) -> Vec<(String, Vec<ConfigValue>)> {
    let mut grid: Vec<(String, Vec<ConfigValue>)> = Vec::new();
    for d in dims {
        let mut cands: Vec<ConfigValue> = Vec::new();
        for v in &d.values {
            let mut m = HashMap::new();
            m.insert(d.key.clone(), v.clone());
            if let Ok(cv) = config_values(&m) {
                if let Some(c) = cv.get(&d.key) {
                    cands.push(c.clone());
                }
            }
        }
        if !cands.is_empty() {
            grid.push((d.key.clone(), cands));
        }
    }
    grid
}

/// 候选组合数(纯函数): 用于向用户**明码标价** —— 形态 B 是 `组合数 × 段数` 次回测。
fn combos_len(dims: &[OptimizeDim]) -> usize {
    crate::commands::optimize::expand_grid(&candidate_grid(dims)).len()
}

/// T31: 形态 B —— 每段用**该段之前**的全部数据重新寻优, 再在紧随其后的一段上验证。
///
/// 两段**严格不重叠**: 训练段 `[total.start, seg.start)`, 验证段 `seg`。
/// 越界(哪怕一天)就会把未来信息漏进寻优, 那整个滚动验证就白做了 —— 这是本函数
/// 唯一不能出错的地方, 所以切点直接复用 `total` 与 `segs[i].start_ms`, 不另算。
///
/// 第一段**不做验证**: 它之前没有数据可供训练(用验证段自己训练 = 直接泄漏答案)。
async fn run_walk_b(
    base: &BacktestRunSpec,
    root: &Arc<std::path::PathBuf>,
    total: Span,
    segs: &[Span],
    dims: &[OptimizeDim],
    metric: &str,
) -> Vec<SegmentOut> {
    let mut out: Vec<SegmentOut> = Vec::new();
    if segs.len() < 2 {
        return out;
    }
    let grid = candidate_grid(dims);
    if grid.is_empty() {
        return out;
    }
    let combos = crate::commands::optimize::expand_grid(&grid);
    if combos.is_empty() || combos.len() > crate::commands::optimize::MAX_COMBOS {
        return out;
    }

    let mut idx = 0usize;
    for seg in segs.iter().skip(1) {
        idx += 1;
        let label = format!("滚动 {idx}/{}", segs.len() - 1);
        let train = Span { start_ms: total.start_ms, end_ms: seg.start_ms };
        let mut best: Option<(f64, Vec<(String, ConfigValue)>)> = None;
        for combo in &combos {
            let mut spec = base.clone();
            spec.root = (**root).clone();
            spec.start = Some(ymd(train.start_ms));
            spec.end = Some(ymd(train.end_ms));
            spec.params = combo.iter().cloned().collect();
            if let Ok(outcome) = run_backtest_full(spec).await {
                let score = score_of(&outcome, metric);
                if best.as_ref().is_none_or(|(b, _)| score > *b) {
                    best = Some((score, combo.clone()));
                }
            }
            // 单组失败跳过(不中断): 一组跑挂不代表整个网格不可用。
        }
        let empty = Vec::new();
        let chosen: &Vec<(String, ConfigValue)> = best.as_ref().map(|(_, c)| c).unwrap_or(&empty);
        let mut spec = base.clone();
        spec.root = (**root).clone();
        spec.start = Some(ymd(seg.start_ms));
        spec.end = Some(ymd(seg.end_ms));
        spec.params = chosen.iter().cloned().collect();
        match run_backtest_full(spec).await {
            Ok(outcome) => {
                let c = outcome.chart();
                let curve = downsample_curve(&c.times, &c.equity);
                out.push(SegmentOut {
                    index: idx,
                    label,
                    start: ymd(seg.start_ms),
                    end: ymd(seg.end_ms),
                    days: seg.days().round().max(1.0) as u32,
                    metrics: Some(serde_json::to_value(outcome.metrics()).unwrap_or(Value::Null)),
                    curve,
                    reason: None,
                })
            }
            Err(e) => out.push(SegmentOut {
                index: idx,
                label,
                start: ymd(seg.start_ms),
                end: ymd(seg.end_ms),
                days: seg.days().round().max(1.0) as u32,
                metrics: None,
                curve: None,
                reason: Some(redact_root(&e.to_string(), root)),
            }),
        }
    }
    out
}

/// 形态 B 的打分(纯函数口径与 CLI `ricow optimize` 一致, 不另造一套)。
fn score_of(out: &BacktestOutcome, metric: &str) -> f64 {
    let r = &out.report;
    match metric {
        "sharpe" => r.sharpe.unwrap_or(f64::NEG_INFINITY),
        "annual_return" => r.annual_return.unwrap_or(f64::NEG_INFINITY),
        _ => r.net_pnl.to_f64().unwrap_or(f64::NEG_INFINITY),
    }
}

fn walk_b_note(segs: usize, combos: usize) -> String {
    format!(
        "滚动验证(含重新调参): 每段的参数都是**只用该段之前的数据**重新寻出来的, \
         每段结果都来自它没见过的数据 —— 这是它比分段验证更接近真实的地方。\
         代价: 每段前都要跑一遍候选网格(本次 {combos} 组 × {segs} 段 ≈ {} 次回测), \
         可能要十几分钟, 慢是正常的。",
        combos * segs
    )
}

fn walk_note(form: &str, n: usize) -> String {
    if form == "B" {
        format!(
            "滚动验证(含重新调参)共 {n} 段: 每段的参数都是**只用该段之前的数据**重新寻出来的, \
             每段结果都来自它没见过的数据。代价是每段都要跑一整轮寻优 —— 这就是它慢的原因。"
        )
    } else {
        format!(
            "分段验证共 {n} 段: 参数**全程不变**(就是你当前这组), 每段独立跑一次。\
             看的是同一组参数在不同行情段上的稳定性 —— 段与段差异大, 说明它对行情很挑。"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(days: i64) -> Span {
        Span { start_ms: 0, end_ms: days * 86_400_000 }
    }

    /// D4: 只能按时间顺序切 —— 前段全部早于后段, 且两段**不重叠**。
    #[test]
    fn test_split_is_chronological_and_covers_everything() {
        let (a, b) = split_window(span(100));
        assert_eq!(a.start_ms, 0);
        assert_eq!(b.end_ms, 100 * 86_400_000);
        assert_eq!(a.end_ms, b.start_ms, "切点必须严丝合缝, 不重叠不留缝");
        assert!(a.days() > b.days(), "前段必须更长(70/30)");
        assert!((a.days() - 70.0).abs() < 0.5 && (b.days() - 30.0).abs() < 0.5);
    }

    /// D5: 门槛是**两条线**, 不是三个并列档位。
    #[test]
    fn test_threshold_is_two_lines_not_three_tiers() {
        assert_eq!(oos_threshold(29.0), "insufficient");
        assert_eq!(oos_threshold(MIN_OOS_DAYS as f64), "minimum");
        assert_eq!(oos_threshold(89.0), "minimum");
        assert_eq!(oos_threshold(AMPLE_OOS_DAYS as f64), "ample");
        // 默认 90 天窗口切完只有 27 天样本外 —— 这个事实必须被暴露出来, 不是缺陷。
        let (_, b) = split_window(span(90));
        assert_eq!(oos_threshold(b.days()), "insufficient", "90 天窗口切完确实不够");
    }

    /// D6: 退化判据 —— 只看"变差", 不出现"过拟合"字样。
    #[test]
    fn test_degraded_rule_and_wording() {
        assert_eq!(is_degraded(Some(100.0), Some(-10.0), Some(5.0), Some(5.0)), Some(true));
        assert_eq!(is_degraded(Some(100.0), Some(10.0), Some(5.0), Some(20.0)), Some(true));
        assert_eq!(is_degraded(Some(100.0), Some(10.0), Some(5.0), Some(6.0)), Some(false));
        // 算不出 = None(不打标), 绝不退化成"没退化"。
        assert_eq!(is_degraded(None, Some(1.0), None, None), None);
        for s in [
            split_note("insufficient", 27, None),
            split_note("minimum", 40, Some(true)),
            split_note("ample", 120, Some(true)),
        ] {
            assert!(!s.contains("过拟合"), "结论不得定性为过拟合: {s}");
        }
        assert!(
            split_note("minimum", 40, Some(true)).contains("样本外退化")
                || split_note("minimum", 40, Some(true)).contains("明显变差")
        );
    }

    /// T30: 段数 = clamp(floor(天数/60), 2, 5), 每段 ≥ 30 天。
    #[test]
    fn test_walk_segment_counts_match_spec_table() {
        assert!(walk_segments(span(30)).is_empty(), "30 天 → 不足");
        assert_eq!(walk_segments(span(60)).len(), 2);
        assert_eq!(walk_segments(span(90)).len(), 2);
        assert_eq!(walk_segments(span(180)).len(), 3);
        assert_eq!(walk_segments(span(360)).len(), 5);
        // 上界: 再长也不会超过 5 段。
        assert_eq!(walk_segments(span(3650)).len(), MAX_SEGMENTS as usize);
        // 每段 ≥ 30 天(最后一段吃掉余数, 只可能更长)。
        for s in walk_segments(span(180)) {
            assert!(s.days() >= MIN_SEGMENT_DAYS as f64 - 0.5, "每段须 ≥ 30 天: {:?}", s);
        }
        // 首尾严丝合缝, 不留缝不重叠。
        let segs = walk_segments(span(360));
        assert_eq!(segs.first().unwrap().start_ms, 0);
        assert_eq!(segs.last().unwrap().end_ms, 360 * 86_400_000);
        for w in segs.windows(2) {
            assert_eq!(w[0].end_ms, w[1].start_ms);
        }
    }

    /// T31: 形态 B 段数压到 3(成本高两个数量级)。
    #[test]
    fn test_walk_b_is_capped_at_three() {
        assert_eq!(walk_b_segments(span(360)).len(), MAX_WALK_B_SEGMENTS as usize);
        assert_eq!(walk_b_segments(span(180)).len(), 3);
    }

    /// T32 术语纪律: A 与 B 不统称 walk-forward, 且形态 B 必须写明"含重新调参"。
    #[test]
    fn test_terminology_never_merges_the_two_forms() {
        let a = walk_note("A", 3);
        let b = walk_note("B", 3);
        assert!(a.contains("分段验证") && !a.contains("重新调参"), "{a}");
        assert!(b.contains("滚动验证") && b.contains("重新调参"), "{b}");
        assert!(!a.contains("walk-forward") && !b.contains("walk-forward"), "界面不许甩术语");
        assert_ne!(segment_label("walk", "A", 0, 3), segment_label("walk", "B", 0, 3));
        // split 的段名要让用户知道哪段是样本外。
        assert!(segment_label("split", "A", 1, 2).contains("样本外"));
    }

    /// T32: 段曲线抽稀 —— 末点必须保留(它是下一段的接点), 长度不一致直接 `None` 而不是错位配对。
    #[test]
    fn test_curve_downsample_keeps_the_last_point() {
        let times: Vec<i64> = (0..600).map(|i| i * 3_600_000).collect();
        let eq: Vec<f64> = (0..600).map(|i| i as f64).collect();
        let v = downsample_curve(&times, &eq).expect("非空序列应有曲线");
        let t = v.get("t").and_then(|x| x.as_array()).unwrap();
        let e = v.get("eq").and_then(|x| x.as_array()).unwrap();
        assert!(t.len() <= CURVE_MAX_POINTS + 1, "抽稀后须受控: {}", t.len());
        assert_eq!(t.len(), e.len(), "时间与权益必须等长");
        assert_eq!(t.last().unwrap().as_i64().unwrap(), times[599]);
        assert_eq!(e.last().unwrap().as_f64().unwrap(), 599.0);
        // 空 / 错位 → None(不给一条错配的曲线)。
        assert!(downsample_curve(&[], &[]).is_none());
        assert!(downsample_curve(&times, &[1.0]).is_none());
    }
}
