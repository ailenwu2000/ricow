//! `ricow pairs` — 交易对视野 (现货 / 合约两组符号) 的列出与检索。
//!
//! 视野默认只含股票类 (019 G7): 现货 bStock (`XxxBUSDT` 形态) + 美股代币永续 (EQUITY 池);
//! `[market] show_all_pairs = true`(或本次 `--all`)放开为币安全量 TRADING。
//!
//! 组装/检索本身是纯函数 (`ricow_engine::market_class`), 本模块只负责: 免 key 网络拉取
//! → 进程内 TTL 缓存 → 打印。三个出口共用同一份视野:
//! ① CLI `ricow pairs`; ② AI 工具 `list_pairs`; ③ 对话内 `/market`。

use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use clap::Args;
use ricow_core::{CoreError, CoreResult, Market};
use ricow_engine::{build_view, filter_view, PairsView};

/// 免 key 市场数据的进程内缓存时长(CLI 每次进程独立, 缓存主要服务对话/工具连续调用)。
pub const CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Args)]
pub struct PairsArgs {
    /// 只看一个市场: spot(现货) / futures(合约); 省略 = 两组都列
    #[arg(long)]
    pub market: Option<String>,
    /// 交易对子串检索 (大小写不敏感), 如 --q AAPL
    #[arg(long)]
    pub q: Option<String>,
    /// 本次列出全部交易对 (不改配置文件; 永久生效请改 [market] show_all_pairs)
    #[arg(long)]
    pub all: bool,
}

pub async fn pairs(args: PairsArgs) -> CoreResult<()> {
    let market = normalize_market(args.market.as_deref())?;
    let root = crate::commands::project_root();
    let view = current_view(&root, args.all).await?;
    let picked = filter_view(&view, market.as_deref(), args.q.as_deref());
    print!("{}", render(&picked, args.q.as_deref()));
    Ok(())
}

/// 校验 `--market` 取值(纯函数): 只认 spot / futures, 拼错硬失败(不静默当作全市场)。
fn normalize_market(v: Option<&str>) -> CoreResult<Option<String>> {
    match v.map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) if s == "spot" || s == "futures" => Ok(Some(s)),
        Some(other) => Err(CoreError::InvalidArgument(format!(
            "--market 只支持 spot / futures; 收到: {other}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// 视野组装 (网络 + 缓存)
// ---------------------------------------------------------------------------

/// 一次性拉取的免 key 市场快照(三个来源同批, 避免视野内自相矛盾)。
struct Snapshot {
    spots: Vec<Market>,
    /// 全永续 (PERPETUAL ∪ TRADIFI_PERPETUAL)。
    futures: Vec<Market>,
    /// fapi EQUITY 池 base(美股代币识别锚点)。
    equity_bases: Vec<String>,
    fetched_at: Instant,
}

static CACHE: Mutex<Option<Arc<Snapshot>>> = Mutex::new(None);

/// 缓存是否仍新鲜(纯函数, 便于单测)。
fn is_fresh(age: Duration, ttl: Duration) -> bool {
    age < ttl
}

fn cache() -> MutexGuard<'static, Option<Arc<Snapshot>>> {
    // 缓存被毒化(某线程取快照时 panic)不该让后续命令全废: 取回内层值继续用。
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

async fn fetch() -> CoreResult<Snapshot> {
    let spot = ricow_binance::BinanceClient::new()?;
    let fapi = ricow_binance::FuturesDataClient::new()?;
    let (spots, futures, equity_bases) = tokio::try_join!(
        spot.get_exchange_info(),
        fapi.get_perp_markets(),
        fapi.get_equity_pool()
    )?;
    Ok(Snapshot { spots, futures, equity_bases, fetched_at: Instant::now() })
}

/// 取市场快照(命中未过期缓存则复用)。
async fn snapshot() -> CoreResult<Arc<Snapshot>> {
    if let Some(hit) = cache().as_ref().filter(|s| is_fresh(s.fetched_at.elapsed(), CACHE_TTL)) {
        return Ok(hit.clone());
    }
    let fresh = Arc::new(fetch().await?);
    *cache() = Some(fresh.clone());
    Ok(fresh)
}

/// 当前视野: `[market] show_all_pairs` 决定过滤/全量; `force_all` = 本次强制全量(不写配置)。
pub async fn current_view(root: &Path, force_all: bool) -> CoreResult<PairsView> {
    let show_all = force_all || crate::commands::config_file::load(root)?.market.show_all_pairs;
    let snap = snapshot().await?;
    Ok(build_view(&snap.spots, &snap.futures, &snap.equity_bases, show_all))
}

/// 视野 + 市场/子串检索(AI 工具与 CLI 共用入口)。
pub async fn lookup(root: &Path, market: Option<&str>, q: Option<&str>) -> CoreResult<PairsView> {
    let view = current_view(root, false).await?;
    Ok(filter_view(&view, market, q))
}

/// 视野正文: 视野说明 + 计数 + 两组符号(每行 6 个, 便于人读; 交由调用方决定截断)。
pub fn render(view: &PairsView, q: Option<&str>) -> String {
    let mut out = String::new();
    line!(out, "交易对视野: {}", scope_text(view.filtered));
    if let Some(q) = q.map(str::trim).filter(|s| !s.is_empty()) {
        line!(out, "检索: {q}(大小写不敏感子串)");
    }
    line!(
        out,
        "现货 {} 个 / 合约 {} 个 / 合计 {}",
        view.spot.len(),
        view.futures.len(),
        view.total()
    );
    if view.filtered {
        line!(
            out,
            "提示: 默认只显示股票类; 全部交易对用 `ricow pairs --all` 或改 [market] show_all_pairs"
        );
    }
    line!(out, "");
    for (label, list) in [("现货", &view.spot), ("合约", &view.futures)] {
        if list.is_empty() {
            line!(out, "[{label}] (无)");
            continue;
        }
        line!(out, "[{label}]");
        for chunk in list.chunks(6) {
            line!(out, "  {}", chunk.join("  "));
        }
    }
    out
}

/// 视野一句话(CLI 与对话共用, 不含列表)。
pub fn scope_text(filtered: bool) -> &'static str {
    if filtered {
        "仅股票类(bStock 美股代币现货 + 股票永续) [默认]"
    } else {
        "全部交易对"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_market_accepts_known_and_rejects_typo() {
        assert_eq!(normalize_market(None).unwrap(), None);
        assert_eq!(normalize_market(Some("  ")).unwrap(), None);
        assert_eq!(normalize_market(Some("SPOT")).unwrap().as_deref(), Some("spot"));
        assert_eq!(normalize_market(Some(" futures ")).unwrap().as_deref(), Some("futures"));
        let err = normalize_market(Some("future")).unwrap_err().to_string();
        assert!(err.contains("spot / futures"), "{err}");
    }

    #[test]
    fn test_cache_freshness_boundary() {
        let ttl = Duration::from_secs(600);
        assert!(is_fresh(Duration::from_secs(0), ttl));
        assert!(is_fresh(Duration::from_secs(599), ttl));
        assert!(!is_fresh(ttl, ttl), "到点即过期");
        assert!(!is_fresh(Duration::from_secs(601), ttl));
    }

    #[test]
    fn test_render_reports_scope_counts_and_hint() {
        let view = PairsView {
            spot: vec!["AAPLBUSDT".into(), "TSLABUSDT".into()],
            futures: vec!["AAPLUSDT".into()],
            filtered: true,
        };
        let text = render(&view, Some("aapl"));
        assert!(text.contains("仅股票类"), "{text}");
        assert!(text.contains("检索: aapl"), "{text}");
        assert!(text.contains("现货 2 个 / 合约 1 个 / 合计 3"), "{text}");
        assert!(text.contains("--all"), "过滤视野要给全量出口: {text}");
        assert!(text.contains("AAPLBUSDT") && text.contains("AAPLUSDT"), "{text}");

        let all = PairsView { spot: vec![], futures: vec![], filtered: false };
        let text = render(&all, None);
        assert!(text.contains("全部交易对"), "{text}");
        assert!(text.contains("[现货] (无)"), "{text}");
        assert!(!text.contains("--all"), "全量视野无需提示切换: {text}");
    }

    /// 真机(免 key 公共行情): 默认视野确为股票类、全量更大、检索可用。
    ///
    /// 纪律: 真实调用, 不 mock。运行:
    /// `cargo test -p ricow --bin ricow pairs::tests::test_pairs_view_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "真机: 需网络访问币安公共行情(免 key)"]
    async fn test_pairs_view_live_stock_scope_and_filter() {
        let snap = snapshot().await.expect("拉取市场快照失败");
        println!(
            "spots={} futures={} equity_bases={}",
            snap.spots.len(),
            snap.futures.len(),
            snap.equity_bases.len()
        );

        let stock = build_view(&snap.spots, &snap.futures, &snap.equity_bases, false);
        let all = build_view(&snap.spots, &snap.futures, &snap.equity_bases, true);
        println!("默认视野: {}\n{}", scope_text(stock.filtered), render(&stock, None));

        assert!(stock.filtered && !all.filtered);
        assert!(!stock.spot.is_empty(), "默认现货视野应含 bStock(XxxBUSDT)");
        assert!(!stock.futures.is_empty(), "默认合约视野应含股票永续");
        // 现货形态: 去尾 B 必在 EQUITY 白名单(证明是交叉过滤而非"以 B 结尾")
        for s in &stock.spot {
            let base = s.strip_suffix("USDT").expect("现货符号以 USDT 计价");
            let stripped = base.strip_suffix('B').expect("bStock 现货 base 以 B 结尾");
            assert!(
                snap.equity_bases.iter().any(|e| e.eq_ignore_ascii_case(stripped)),
                "{s} 的 base 去尾 B 后不在 EQUITY 白名单"
            );
        }
        assert!(all.total() >= stock.total(), "全量视野不应少于过滤视野");
        // 检索: AAPL 在两组都应命中(若该股票已上线)
        let hit = filter_view(&stock, None, Some("aapl"));
        println!("检索 aapl: 现货 {:?} / 合约 {:?}", hit.spot, hit.futures);
        assert!(hit.total() <= stock.total());
    }
}
