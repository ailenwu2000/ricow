//! SQLite 本地数据持久化: K 线 / 成交 / PnL 快照。

use std::path::Path;
use std::str::FromStr;

use chrono::DateTime;
use ricow_core::{Kline, OrderFill};
use rust_decimal::Decimal;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

/// 数据库 — 封装 SQLite 连接池与持久化操作。
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

/// 成交查询结果行 (含策略标识, 供 `ricow fills` / `ricow info` 使用)。
#[derive(Debug, Clone, PartialEq)]
pub struct FillRecord {
    pub strategy_id: String,
    pub pair: String,
    pub side: String,
    pub fill_price: Decimal,
    pub fill_size: Decimal,
    pub fee: Decimal,
    pub timestamp: i64,
}

impl Database {
    /// 打开 (如不存在则创建) 指定路径的 SQLite 数据库, 并运行建表迁移。
    pub async fn open(path: &Path) -> Result<Self, sqlx::Error> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(sqlx::Error::Io)?;
            }
        }
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    /// 内存数据库 (测试用)。
    pub async fn open_in_memory() -> Result<Self, sqlx::Error> {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS klines (
                pair TEXT NOT NULL,
                interval TEXT NOT NULL,
                open_time INTEGER NOT NULL,
                open TEXT NOT NULL,
                high TEXT NOT NULL,
                low TEXT NOT NULL,
                close TEXT NOT NULL,
                volume TEXT NOT NULL,
                close_time INTEGER NOT NULL,
                PRIMARY KEY (pair, interval, open_time)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS fills (
                trade_id TEXT,
                exchange_order_id TEXT NOT NULL,
                client_order_id TEXT NOT NULL,
                strategy_id TEXT NOT NULL DEFAULT '',
                pair TEXT NOT NULL,
                side TEXT NOT NULL,
                fill_price TEXT NOT NULL,
                fill_size TEXT NOT NULL,
                fee TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // 美股日线缓存 (Nasdaq 信号数据源, 与币安 klines 表隔离 — 005-market-filter)。
        // pair = 美股代码 (TSLA/SPY), interval 恒 '1d'; 主键含 ticker 防跨市场污染。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS us_klines (
                ticker TEXT NOT NULL,
                interval TEXT NOT NULL,
                open_time INTEGER NOT NULL,
                open TEXT NOT NULL,
                high TEXT NOT NULL,
                low TEXT NOT NULL,
                close TEXT NOT NULL,
                volume TEXT NOT NULL,
                close_time INTEGER NOT NULL,
                PRIMARY KEY (ticker, interval, open_time)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pnl_snapshots (
                strategy_id TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                realized_pnl TEXT NOT NULL,
                fees TEXT NOT NULL,
                net_pnl TEXT NOT NULL,
                trade_count INTEGER NOT NULL,
                PRIMARY KEY (strategy_id, timestamp)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS previews (
                preview_id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                status TEXT NOT NULL,
                token TEXT,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // 资金费流水 (014): 以交易所账单为准; tran_id 为账户级唯一流水号 → 幂等重放安全。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS funding_fees (
                tran_id TEXT PRIMARY KEY,
                strategy_id TEXT NOT NULL,
                symbol TEXT NOT NULL,
                income TEXT NOT NULL,
                asset TEXT NOT NULL,
                funding_time INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_fills_timestamp ON fills(timestamp)")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // ---- K 线 ----

    /// 插入或忽略一条 K 线 (主键冲突时跳过, 增量去重)。
    pub async fn insert_kline(
        &self,
        pair: &str,
        interval: &str,
        k: &Kline,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT OR IGNORE INTO klines (pair, interval, open_time, open, high, low, close, volume, close_time)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(pair)
        .bind(interval)
        .bind(k.open_time.timestamp_millis())
        .bind(k.open.to_string())
        .bind(k.high.to_string())
        .bind(k.low.to_string())
        .bind(k.close.to_string())
        .bind(k.volume.to_string())
        .bind(k.close_time.timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 查询 K 线 (按 open_time 升序, 最近 limit 条)。
    pub async fn get_klines(
        &self,
        pair: &str,
        interval: &str,
        limit: u32,
    ) -> Result<Vec<Kline>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT open_time, open, high, low, close, volume, close_time
             FROM klines WHERE pair = ? AND interval = ?
             ORDER BY open_time DESC LIMIT ?",
        )
        .bind(pair)
        .bind(interval)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut klines: Vec<Kline> = rows
            .iter()
            .filter_map(|r| {
                let open_time = r.get::<i64, _>("open_time");
                let close_time = r.get::<i64, _>("close_time");
                Some(Kline {
                    open_time: DateTime::from_timestamp_millis(open_time)?,
                    open: Decimal::from_str(r.get::<String, _>("open").as_str()).ok()?,
                    high: Decimal::from_str(r.get::<String, _>("high").as_str()).ok()?,
                    low: Decimal::from_str(r.get::<String, _>("low").as_str()).ok()?,
                    close: Decimal::from_str(r.get::<String, _>("close").as_str()).ok()?,
                    volume: Decimal::from_str(r.get::<String, _>("volume").as_str()).ok()?,
                    close_time: DateTime::from_timestamp_millis(close_time)?,
                })
            })
            .collect();
        // 反转为升序
        klines.reverse();
        Ok(klines)
    }

    /// K 线总数。
    pub async fn kline_count(&self) -> Result<i64, sqlx::Error> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM klines").fetch_one(&self.pool).await?;
        Ok(count)
    }

    // ---- 美股日线 (us_klines, Nasdaq 信号数据源) ----

    /// 插入或忽略一条美股日线 (主键 = ticker+interval+open_time, 增量去重)。
    pub async fn insert_us_kline(
        &self,
        ticker: &str,
        k: &Kline,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT OR IGNORE INTO us_klines (ticker, interval, open_time, open, high, low, close, volume, close_time)
             VALUES (?, '1d', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(ticker)
        .bind(k.open_time.timestamp_millis())
        .bind(k.open.to_string())
        .bind(k.high.to_string())
        .bind(k.low.to_string())
        .bind(k.close.to_string())
        .bind(k.volume.to_string())
        .bind(k.close_time.timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 批量插入美股日线 (单事务, 2026-09-09 bs_momentum 组合入口用)。
    /// 首拉 70 只 × ~2500 根 ≈ 17.5 万行 — 逐条独立事务为分钟级, 单事务毫秒级
    /// (SQLite WAL; INSERT OR IGNORE 增量去重, 已有缓存补拉安全)。
    pub async fn insert_us_klines(
        &self,
        ticker: &str,
        klines: &[Kline],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for k in klines {
            sqlx::query(
                "INSERT OR IGNORE INTO us_klines (ticker, interval, open_time, open, high, low, close, volume, close_time)
                 VALUES (?, '1d', ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(ticker)
            .bind(k.open_time.timestamp_millis())
            .bind(k.open.to_string())
            .bind(k.high.to_string())
            .bind(k.low.to_string())
            .bind(k.close.to_string())
            .bind(k.volume.to_string())
            .bind(k.close_time.timestamp_millis())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// 查询美股日线 (升序, 最近 limit 条)。
    pub async fn get_us_klines(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<Kline>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT open_time, open, high, low, close, volume, close_time
             FROM us_klines WHERE ticker = ? AND interval = '1d'
             ORDER BY open_time DESC LIMIT ?",
        )
        .bind(ticker)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut klines: Vec<Kline> = rows
            .iter()
            .filter_map(|r| {
                let open_time = r.get::<i64, _>("open_time");
                let close_time = r.get::<i64, _>("close_time");
                Some(Kline {
                    open_time: DateTime::from_timestamp_millis(open_time)?,
                    open: Decimal::from_str(r.get::<String, _>("open").as_str()).ok()?,
                    high: Decimal::from_str(r.get::<String, _>("high").as_str()).ok()?,
                    low: Decimal::from_str(r.get::<String, _>("low").as_str()).ok()?,
                    close: Decimal::from_str(r.get::<String, _>("close").as_str()).ok()?,
                    volume: Decimal::from_str(r.get::<String, _>("volume").as_str()).ok()?,
                    close_time: DateTime::from_timestamp_millis(close_time)?,
                })
            })
            .collect();
        klines.reverse();
        Ok(klines)
    }

    // ---- 成交 ----

    /// 记录一笔成交。
    pub async fn insert_fill(
        &self,
        strategy_id: &str,
        fill: &OrderFill,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO fills (trade_id, exchange_order_id, client_order_id, strategy_id, pair, side, fill_price, fill_size, fee, timestamp)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(fill.trade_id.as_deref())
        .bind(&fill.exchange_order_id)
        .bind(&fill.client_order_id)
        .bind(strategy_id)
        .bind(&fill.pair)
        .bind(fill.side.to_string())
        .bind(fill.fill_price.to_string())
        .bind(fill.fill_size.to_string())
        .bind(fill.fee.to_string())
        .bind(fill.timestamp.timestamp_millis())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 成交总数。
    pub async fn fill_count(&self) -> Result<i64, sqlx::Error> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM fills").fetch_one(&self.pool).await?;
        Ok(count)
    }

    /// 近期成交 (按时间倒序; `strategy_id=None` 表示全部策略)。
    pub async fn recent_fills(
        &self,
        strategy_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<FillRecord>, sqlx::Error> {
        let rows = match strategy_id {
            Some(id) => {
                sqlx::query(
                    "SELECT strategy_id, pair, side, fill_price, fill_size, fee, timestamp
                     FROM fills WHERE strategy_id = ? ORDER BY timestamp DESC LIMIT ?",
                )
                .bind(id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT strategy_id, pair, side, fill_price, fill_size, fee, timestamp
                     FROM fills ORDER BY timestamp DESC LIMIT ?",
                )
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| FillRecord {
                strategy_id: r.get("strategy_id"),
                pair: r.get("pair"),
                side: r.get("side"),
                fill_price: Decimal::from_str(r.get::<String, _>("fill_price").as_str())
                    .unwrap_or_default(),
                fill_size: Decimal::from_str(r.get::<String, _>("fill_size").as_str())
                    .unwrap_or_default(),
                fee: Decimal::from_str(r.get::<String, _>("fee").as_str()).unwrap_or_default(),
                timestamp: r.get("timestamp"),
            })
            .collect())
    }

    // ---- 资金费 (014) ----

    /// 写入一条资金费流水 (014 FR-002): `tran_id` 是交易所**账户级唯一**流水号 → 重复写入被忽略(幂等)。
    /// 返回 `true` = 本次新插入。
    pub async fn insert_funding_fee(
        &self,
        strategy_id: &str,
        f: &ricow_core::FundingIncome,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "INSERT OR IGNORE INTO funding_fees (tran_id, strategy_id, symbol, income, asset, funding_time)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&f.tran_id)
        .bind(strategy_id)
        .bind(&f.symbol)
        .bind(f.income.to_string())
        .bind(&f.asset)
        .bind(f.time_ms)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// 累计资金费 (014 FR-004): `strategy_id = None` → 账户级全部流水(推荐, 资金费本是账户级事实)。
    /// Rust 侧 `Decimal` 精确聚合 —— 不用 SQL `SUM`(REAL 往返污染小数, 见 012 教训)。
    pub async fn funding_total(
        &self,
        strategy_id: Option<&str>,
    ) -> Result<(i64, Decimal), sqlx::Error> {
        let rows = match strategy_id {
            Some(s) => {
                sqlx::query("SELECT income FROM funding_fees WHERE strategy_id = ?")
                    .bind(s)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => sqlx::query("SELECT income FROM funding_fees").fetch_all(&self.pool).await?,
        };
        let mut sum = Decimal::ZERO;
        for r in &rows {
            let raw: String = r.get("income");
            if let Ok(d) = Decimal::from_str(raw.as_str()) {
                sum += d;
            }
        }
        Ok((rows.len() as i64, sum))
    }

    /// 增量拉取水位: 最近一条资金费结算时间(毫秒); 无记录 → None(跨进程重启自然续拉)。
    pub async fn latest_funding_time(&self) -> Result<Option<i64>, sqlx::Error> {
        let row = sqlx::query("SELECT MAX(funding_time) AS t FROM funding_fees")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<Option<i64>, _>("t"))
    }

    /// 单策略成交统计: (笔数, 手续费合计, 最近成交时间毫秒)。
    pub async fn fill_stats(
        &self,
        strategy_id: &str,
    ) -> Result<(i64, Decimal, Option<i64>), sqlx::Error> {
        // 手续费在 Rust 侧用 `Decimal` 精确累加, 不用 SQL `SUM(CAST(... AS REAL))`:
        //  ① REAL 往返会把 0.000004 变成 0.0000040000000000000002081668171 (f64 噪音, 账目数字不接受);
        //  ② 无成交时 COALESCE(..., 0) 兜底出 INTEGER, 按 f64 解码会直接 panic (2026-09-13 实测 `ricow info` 崩溃)。
        let rows = sqlx::query("SELECT fee, timestamp FROM fills WHERE strategy_id = ?")
            .bind(strategy_id)
            .fetch_all(&self.pool)
            .await?;
        let mut fees = Decimal::ZERO;
        let mut last_ts: Option<i64> = None;
        for r in &rows {
            let raw: String = r.get("fee");
            if let Ok(d) = Decimal::from_str(raw.as_str()) {
                fees += d;
            }
            let ts: i64 = r.get("timestamp");
            last_ts = Some(last_ts.map_or(ts, |cur| cur.max(ts)));
        }
        Ok((rows.len() as i64, fees, last_ts))
    }

    // ---- 两步确认 preview ----

    /// 插入一条 preview 记录。
    pub async fn insert_preview(&self, rec: &PreviewRecord) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO previews (preview_id, kind, payload_json, status, token, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&rec.preview_id)
        .bind(&rec.kind)
        .bind(&rec.payload_json)
        .bind(&rec.status)
        .bind(&rec.token)
        .bind(rec.created_at)
        .bind(rec.expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 按 preview_id 查询 preview 记录。
    /// 清理预览记录(019 T040 / FR-046): 删除**已过期**与**终态**(consumed/rejected)行。
    ///
    /// 迭代(每次 `create`/`preview_strategy` 都插一行)会让本表单调增长 —— 本函数在任何新预览
    /// 生成前调用一次, 保证行数不随迭代无限增长。**不新增表**(spec FR-046)。
    /// 返回删除行数。
    pub async fn prune_previews(&self, now_ts: i64) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(
            "DELETE FROM previews WHERE expires_at < ? OR status IN ('consumed', 'rejected')",
        )
        .bind(now_ts)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    pub async fn get_preview(&self, preview_id: &str) -> Result<Option<PreviewRecord>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT preview_id, kind, payload_json, status, token, created_at, expires_at
             FROM previews WHERE preview_id = ?",
        )
        .bind(preview_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| PreviewRecord {
            preview_id: r.get("preview_id"),
            kind: r.get("kind"),
            payload_json: r.get("payload_json"),
            status: r.get("status"),
            token: r.get("token"),
            created_at: r.get("created_at"),
            expires_at: r.get("expires_at"),
        }))
    }

    /// 条件更新 preview 状态 (原子 CAS): 仅当当前状态为 `expect_status` 时更新。
    ///
    /// 返回受影响行数 (0 = 状态已被并发修改, 调用方应视为竞争失败)。
    pub async fn update_preview(
        &self,
        preview_id: &str,
        expect_status: &str,
        status: &str,
        token: Option<&str>,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE previews SET status = ?, token = ? WHERE preview_id = ? AND status = ?",
        )
        .bind(status)
        .bind(token)
        .bind(preview_id)
        .bind(expect_status)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

/// preview 记录 (两步确认)。
#[derive(Debug, Clone)]
pub struct PreviewRecord {
    pub preview_id: String,
    pub kind: String,
    pub payload_json: String,
    pub status: String,
    pub token: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn test_prune_previews_removes_expired_and_terminal_only() {
        let db = Database::open_in_memory().await.unwrap();
        let now = 1_000_000_i64;
        let mk = |id: &str, status: &str, expires: i64| PreviewRecord {
            preview_id: id.to_string(),
            kind: "strategy".into(),
            payload_json: "{}".into(),
            status: status.to_string(),
            token: None,
            created_at: now - 10,
            expires_at: expires,
        };
        db.insert_preview(&mk("live-pending", "pending", now + 900)).await.unwrap();
        db.insert_preview(&mk("expired-pending", "pending", now - 1)).await.unwrap();
        db.insert_preview(&mk("done-consumed", "consumed", now + 900)).await.unwrap();
        db.insert_preview(&mk("done-rejected", "rejected", now + 900)).await.unwrap();
        let removed = db.prune_previews(now).await.unwrap();
        assert_eq!(removed, 3, "应只留未过期且非终态的那一行");
        let left = db.get_preview("live-pending").await.unwrap();
        assert!(left.is_some(), "未过期的 pending 必须保留");
        assert!(db.get_preview("expired-pending").await.unwrap().is_none());
        assert!(db.get_preview("done-consumed").await.unwrap().is_none());
    }
    use super::*;
    use chrono::Utc;
    use ricow_core::{FundingIncome, OrderSide};
    use rust_decimal_macros::dec;

    fn sample_kline(t: i64) -> Kline {
        Kline {
            open_time: DateTime::from_timestamp_millis(t).unwrap(),
            open: dec!(3000),
            high: dec!(3010),
            low: dec!(2990),
            close: dec!(3005),
            volume: dec!(100),
            close_time: DateTime::from_timestamp_millis(t + 60_000).unwrap(),
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_klines() {
        let db = Database::open_in_memory().await.unwrap();
        db.insert_kline("ETH", "1m", &sample_kline(1000)).await.unwrap();
        db.insert_kline("ETH", "1m", &sample_kline(1_060_000)).await.unwrap();
        // 重复插入被去重
        db.insert_kline("ETH", "1m", &sample_kline(1000)).await.unwrap();

        let klines = db.get_klines("ETH", "1m", 10).await.unwrap();
        assert_eq!(klines.len(), 2);
        assert_eq!(db.kline_count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_insert_fill() {
        let db = Database::open_in_memory().await.unwrap();
        let fill = OrderFill {
            trade_id: Some("t1".into()),
            exchange_order_id: "e1".into(),
            client_order_id: "c1".into(),
            pair: "ETH".into(),
            side: OrderSide::Buy,
            fill_price: dec!(3000),
            fill_size: dec!(1),
            fee: dec!(0.6),
            timestamp: Utc::now(),
        };
        db.insert_fill("grid-1", &fill).await.unwrap();
        assert_eq!(db.fill_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_funding_fees_idempotent_and_exact_sum() {
        let db = Database::open_in_memory().await.unwrap();
        let f = FundingIncome {
            symbol: "ETHUSDT".into(),
            income: dec!(-0.12345678),
            asset: "USDT".into(),
            time_ms: 1000,
            tran_id: "t1".into(),
        };
        assert!(db.insert_funding_fee("s1", &f).await.unwrap(), "首次插入");
        assert!(!db.insert_funding_fee("s1", &f).await.unwrap(), "重复 tran_id 幂等");
        assert!(!db.insert_funding_fee("s2", &f).await.unwrap(), "跨策略同一流水同样去重(账户级唯一)");
        assert_eq!(db.funding_total(None).await.unwrap(), (1, dec!(-0.12345678)));
        // 精确小数: 加分不再有 f64 尾差
        let f2 = FundingIncome { tran_id: "t2".into(), income: dec!(0.000004), time_ms: 2000, ..f.clone() };
        db.insert_funding_fee("s1", &f2).await.unwrap();
        assert_eq!(db.funding_total(None).await.unwrap(), (2, dec!(-0.12345278)));
        assert_eq!(db.latest_funding_time().await.unwrap(), Some(2000));
        // per-strategy 过滤 (s2 只是"重复被去重"的写入者, 无自己的记录)
        assert_eq!(db.funding_total(Some("s2")).await.unwrap(), (0, dec!(0)));
        // 空表
        let db2 = Database::open_in_memory().await.unwrap();
        assert_eq!(db2.funding_total(None).await.unwrap(), (0, dec!(0)));
        assert!(db2.latest_funding_time().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_fill_stats_tolerates_integer_sum_and_empty() {
        let db = Database::open_in_memory().await.unwrap();
        // 无成交: COALESCE 兜底值不得让 f64 解码 panic (012 实测 `ricow info` 崩溃点)
        let (n, fees, last) = db.fill_stats("no-such-strategy").await.unwrap();
        assert_eq!(n, 0);
        assert_eq!(fees, dec!(0));
        assert!(last.is_none());

        // 零手续费成交: SQLite SUM 走整数路径, 同样不得 panic
        let fill = OrderFill {
            trade_id: Some("t0".into()),
            exchange_order_id: "e0".into(),
            client_order_id: "c0".into(),
            pair: "ETHUSDT".into(),
            side: OrderSide::Sell,
            fill_price: dec!(2500),
            fill_size: dec!(0.04),
            fee: dec!(0),
            timestamp: Utc::now(),
        };
        db.insert_fill("s-zero-fee", &fill).await.unwrap();
        let (n2, fees2, last2) = db.fill_stats("s-zero-fee").await.unwrap();
        assert_eq!(n2, 1);
        assert_eq!(fees2, dec!(0));
        assert!(last2.is_some());

        // 有小数手续费: 合计正确
        let fill2 = OrderFill { trade_id: Some("t1".into()), fee: dec!(0.000004), ..fill };
        db.insert_fill("s-frac", &fill2).await.unwrap();
        let (n3, fees3, _) = db.fill_stats("s-frac").await.unwrap();
        assert_eq!(n3, 1);
        assert_eq!(fees3, dec!(0.000004));
    }

    #[tokio::test]
    async fn test_insert_and_get_us_klines() {
        let db = Database::open_in_memory().await.unwrap();
        // 美股日线独立表: 与币安 klines 隔离 (同名时间戳互不污染)。
        db.insert_us_kline("TSLA", &sample_kline(1000)).await.unwrap();
        db.insert_us_kline("TSLA", &sample_kline(1_060_000)).await.unwrap();
        db.insert_us_kline("TSLA", &sample_kline(1000)).await.unwrap(); // 去重
        db.insert_kline("TSLA", "1d", &sample_kline(1000)).await.unwrap(); // 币安侧同 key 独立

        let us = db.get_us_klines("TSLA", 10).await.unwrap();
        assert_eq!(us.len(), 2, "us_klines 只含美股插入");
        assert_eq!(us[0].open_time.timestamp_millis(), 1000);
        assert_eq!(us[1].open_time.timestamp_millis(), 1_060_000);
        // 币安 klines 表不受 us_klines 影响。
        let bn = db.get_klines("TSLA", "1d", 10).await.unwrap();
        assert_eq!(bn.len(), 1);
        // 空查询安全。
        let empty = db.get_us_klines("AAPL", 10).await.unwrap();
        assert!(empty.is_empty());
    }
}
