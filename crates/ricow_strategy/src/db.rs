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

/// 订单当前状态行 (026 FR-001): 一单一行 upsert, 不是流水。
///
/// `mode` = 运行模式 (`dry_run` / `demo` / `live`), 落在**新增表**而不是既有 `fills` 表 (D5)。
#[derive(Debug, Clone, PartialEq)]
pub struct OrderRecord {
    pub strategy_id: String,
    pub exchange_order_id: String,
    pub client_order_id: String,
    pub pair: String,
    pub side: String,
    pub price: Decimal,
    pub size: Decimal,
    pub filled_size: Decimal,
    pub status: String,
    pub mode: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 当前持仓行 (026 FR-002): 每策略每交易对一行, 表示**当前**持仓, 不是流水。
#[derive(Debug, Clone, PartialEq)]
pub struct PositionRecord {
    pub strategy_id: String,
    pub pair: String,
    pub size: Decimal,
    pub entry_price: Decimal,
    pub mode: String,
    pub updated_at: i64,
}

/// PnL 快照行 (026 FR-007): 每笔成交后一条, 永久保留。
#[derive(Debug, Clone, PartialEq)]
pub struct PnlSnapshotRecord {
    pub strategy_id: String,
    pub timestamp: i64,
    pub realized_pnl: Decimal,
    pub fees: Decimal,
    pub net_pnl: Decimal,
    pub trade_count: i64,
}

/// 成交 + 运行模式 (026 D5): `mode` 由 `orders` 按 `exchange_order_id` 关联带出;
/// 关联不到 → `None`(面板 / 工具**如实**显示"未知", 不猜)。
#[derive(Debug, Clone, PartialEq)]
pub struct FillWithMode {
    pub strategy_id: String,
    pub pair: String,
    pub side: String,
    pub fill_price: Decimal,
    pub fill_size: Decimal,
    pub fee: Decimal,
    pub timestamp: i64,
    pub mode: Option<String>,
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

    /// 关闭连接池 → 之后**任何**读写都报错 (测试支撑)。
    ///
    /// 用于验证 026 FR-004 / SC-003「写库失败不得改变交易主流程」: 生产路径不会调用它。
    #[doc(hidden)]
    pub async fn close_pool_for_test(&self) {
        self.pool.close().await;
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

        // Web UI 会话 (025 / FR-017): 会话表 + 消息表; 均为幂等追加, 既有表零改动。
        // 消息表带 `级别` 列(FR-018): 宿主侧显式标注的 Severity 原样落库, 重开页面按原级别着色。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_sessions (
                session_id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_messages (
                message_id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES web_sessions(session_id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                severity TEXT NOT NULL DEFAULT 'normal',
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // 左侧列表按最后活动倒序 → 覆盖该排序; 消息按会话取全量/最近 N 轮 → (session_id, message_id)。
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_web_sessions_updated ON web_sessions(updated_at)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_web_messages_session ON web_messages(session_id, message_id)",
        )
        .execute(&self.pool)
        .await?;

        // 交易可见性 (026 FR-001/FR-002): 新增 orders / positions 两表, 幂等追加, 既有 8 张表零改动。
        // `mode` 落在新增表(不在既有 `fills` 表加列, 见 D5) —— 成交的 mode 由 exchange_order_id 关联带出。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS orders (
                strategy_id TEXT NOT NULL,
                exchange_order_id TEXT NOT NULL,
                client_order_id TEXT NOT NULL DEFAULT '',
                pair TEXT NOT NULL,
                side TEXT NOT NULL,
                price TEXT NOT NULL,
                size TEXT NOT NULL,
                filled_size TEXT NOT NULL,
                status TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (strategy_id, exchange_order_id)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS positions (
                strategy_id TEXT NOT NULL,
                pair TEXT NOT NULL,
                size TEXT NOT NULL,
                entry_price TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (strategy_id, pair)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_orders_strategy_updated ON orders(strategy_id, updated_at)",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_positions_strategy ON positions(strategy_id)")
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
    pub async fn insert_us_kline(&self, ticker: &str, k: &Kline) -> Result<(), sqlx::Error> {
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
    pub async fn get_us_klines(&self, ticker: &str, limit: u32) -> Result<Vec<Kline>, sqlx::Error> {
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

    // ---- 订单 / 持仓 / PnL (026 交易可见性) ----

    /// upsert 一条订单当前状态 (026 FR-003 时点①②③): 一单一行, 冲突时更新状态与成交量, **保留首次 `created_at`**。
    ///
    /// `price` / `size`(= 委托价/**委托量**) **一经提交不再改写**: 它们是下单时的事实,
    /// 时点②③ 的数据源(`OrderFill` / `OrderUpdate`)里根本没有委托价/委托量, 若允许覆盖就会把
    /// 一单一行改成失真数据(成交价 ≠ 委托价)。成交价在 `fills` 表里, 两者不混。
    pub async fn upsert_order(&self, rec: &OrderRecord) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO orders (strategy_id, exchange_order_id, client_order_id, pair, side, price, size, filled_size, status, mode, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(strategy_id, exchange_order_id) DO UPDATE SET
                client_order_id = excluded.client_order_id,
                pair = excluded.pair,
                -- 空串 = 未知 (时点③ 的 `OrderUpdate` 不带方向): 保留既有已知方向, 不用未知覆盖已知
                side = CASE WHEN excluded.side = '' THEN orders.side ELSE excluded.side END,
                filled_size = excluded.filled_size,
                status = excluded.status,
                mode = excluded.mode,
                updated_at = excluded.updated_at",
        )
        .bind(&rec.strategy_id)
        .bind(&rec.exchange_order_id)
        .bind(&rec.client_order_id)
        .bind(&rec.pair)
        .bind(&rec.side)
        .bind(rec.price.to_string())
        .bind(rec.size.to_string())
        .bind(rec.filled_size.to_string())
        .bind(&rec.status)
        .bind(&rec.mode)
        .bind(rec.created_at)
        .bind(rec.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// upsert 一条当前持仓 (026 FR-003 时点④): 每策略每交易对一行。
    pub async fn upsert_position(&self, rec: &PositionRecord) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO positions (strategy_id, pair, size, entry_price, mode, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(strategy_id, pair) DO UPDATE SET
                size = excluded.size,
                entry_price = excluded.entry_price,
                mode = excluded.mode,
                updated_at = excluded.updated_at",
        )
        .bind(&rec.strategy_id)
        .bind(&rec.pair)
        .bind(rec.size.to_string())
        .bind(rec.entry_price.to_string())
        .bind(&rec.mode)
        .bind(rec.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 写入一条 PnL 快照 (026 FR-007): 每笔成交后一条, 永久保留(FR-009, 无 TTL / 无淘汰)。
    /// 主键 `(strategy_id, timestamp)` 为既有表结构(不改列) → 同毫秒多笔成交时后者覆盖前者。
    pub async fn insert_pnl_snapshot(&self, rec: &PnlSnapshotRecord) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT OR REPLACE INTO pnl_snapshots (strategy_id, timestamp, realized_pnl, fees, net_pnl, trade_count)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&rec.strategy_id)
        .bind(rec.timestamp)
        .bind(rec.realized_pnl.to_string())
        .bind(rec.fees.to_string())
        .bind(rec.net_pnl.to_string())
        .bind(rec.trade_count)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 近期订单 (026 FR-010): 按 `updated_at` 倒序; `strategy_id=None` 表示全部策略。
    pub async fn recent_orders(
        &self,
        strategy_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<OrderRecord>, sqlx::Error> {
        let select = "SELECT strategy_id, exchange_order_id, client_order_id, pair, side, price, size, filled_size, status, mode, created_at, updated_at FROM orders";
        let rows = match strategy_id {
            Some(id) => {
                sqlx::query(&format!(
                    "{select} WHERE strategy_id = ? ORDER BY updated_at DESC LIMIT ?"
                ))
                .bind(id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(&format!("{select} ORDER BY updated_at DESC LIMIT ?"))
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        Ok(rows.iter().map(order_record_from_row).collect())
    }

    /// 当前持仓 (026 FR-010): 按策略过滤。
    pub async fn current_positions(
        &self,
        strategy_id: Option<&str>,
    ) -> Result<Vec<PositionRecord>, sqlx::Error> {
        let rows = match strategy_id {
            Some(id) => {
                sqlx::query(
                    "SELECT strategy_id, pair, size, entry_price, mode, updated_at
                     FROM positions WHERE strategy_id = ? ORDER BY pair",
                )
                .bind(id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT strategy_id, pair, size, entry_price, mode, updated_at
                     FROM positions ORDER BY strategy_id, pair",
                )
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| PositionRecord {
                strategy_id: r.get("strategy_id"),
                pair: r.get("pair"),
                size: dec_of(r, "size"),
                entry_price: dec_of(r, "entry_price"),
                mode: r.get("mode"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    /// 近期 PnL 快照 (026 FR-010): 按时间倒序。
    pub async fn recent_pnl_snapshots(
        &self,
        strategy_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<PnlSnapshotRecord>, sqlx::Error> {
        let rows = match strategy_id {
            Some(id) => {
                sqlx::query(
                    "SELECT strategy_id, timestamp, realized_pnl, fees, net_pnl, trade_count
                     FROM pnl_snapshots WHERE strategy_id = ? ORDER BY timestamp DESC LIMIT ?",
                )
                .bind(id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT strategy_id, timestamp, realized_pnl, fees, net_pnl, trade_count
                     FROM pnl_snapshots ORDER BY timestamp DESC LIMIT ?",
                )
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| PnlSnapshotRecord {
                strategy_id: r.get("strategy_id"),
                timestamp: r.get("timestamp"),
                realized_pnl: dec_of(r, "realized_pnl"),
                fees: dec_of(r, "fees"),
                net_pnl: dec_of(r, "net_pnl"),
                trade_count: r.get("trade_count"),
            })
            .collect())
    }

    /// 近期成交**带运行模式** (026 D5 / FR-005): `fills` ⟕ `orders` 取 `mode`。
    /// 关联不到 → `mode = None`(如实显示"未知", 不猜)。
    pub async fn recent_fills_with_mode(
        &self,
        strategy_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<FillWithMode>, sqlx::Error> {
        let select = "SELECT f.strategy_id AS strategy_id, f.pair AS pair, f.side AS side, f.fill_price AS fill_price, f.fill_size AS fill_size, f.fee AS fee, f.timestamp AS timestamp, o.mode AS mode
                      FROM fills f LEFT JOIN orders o
                        ON f.strategy_id = o.strategy_id AND f.exchange_order_id = o.exchange_order_id";
        let rows = match strategy_id {
            Some(id) => {
                sqlx::query(&format!(
                    "{select} WHERE f.strategy_id = ? ORDER BY f.timestamp DESC LIMIT ?"
                ))
                .bind(id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(&format!("{select} ORDER BY f.timestamp DESC LIMIT ?"))
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        Ok(rows
            .iter()
            .map(|r| FillWithMode {
                strategy_id: r.get("strategy_id"),
                pair: r.get("pair"),
                side: r.get("side"),
                fill_price: dec_of(r, "fill_price"),
                fill_size: dec_of(r, "fill_size"),
                fee: dec_of(r, "fee"),
                timestamp: r.get("timestamp"),
                mode: r.get::<Option<String>, _>("mode"),
            })
            .collect())
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

    pub async fn get_preview(
        &self,
        preview_id: &str,
    ) -> Result<Option<PreviewRecord>, sqlx::Error> {
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

    // ---- Web UI 会话 (025) ----

    /// 新建一个 Web 会话; 标题留空(首条用户消息到达时由 [`Database::set_web_title_if_empty`] 生成)。
    pub async fn create_web_session(
        &self,
        session_id: &str,
        now_ts: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO web_sessions (session_id, title, created_at, updated_at) VALUES (?, '', ?, ?)",
        )
        .bind(session_id)
        .bind(now_ts)
        .bind(now_ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 列出全部会话(左侧列表): 按最后活动时间倒序。
    pub async fn list_web_sessions(&self) -> Result<Vec<WebSessionRecord>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT session_id, title, created_at, updated_at
             FROM web_sessions ORDER BY updated_at DESC, session_id DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| WebSessionRecord {
                session_id: r.get("session_id"),
                title: r.get("title"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    /// 删除会话; 其消息由外键 `ON DELETE CASCADE` 一并删除(FR-019, 不残留孤儿消息)。
    /// 返回删除的会话行数(0 = 该会话不存在)。
    pub async fn delete_web_session(&self, session_id: &str) -> Result<u64, sqlx::Error> {
        let r = sqlx::query("DELETE FROM web_sessions WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// 仅在标题为空时写入(FR-022 首条用户消息自动生成标题; 幂等, 不覆盖已有标题)。
    /// 返回受影响行数(0 = 会话不存在或标题已存在)。
    pub async fn set_web_title_if_empty(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<u64, sqlx::Error> {
        let r =
            sqlx::query("UPDATE web_sessions SET title = ? WHERE session_id = ? AND title = ''")
                .bind(title)
                .bind(session_id)
                .execute(&self.pool)
                .await?;
        Ok(r.rows_affected())
    }

    /// 追加一条消息, 同时把所属会话的最后活动时间推到 `now_ts`(左侧列表排序依据)。
    pub async fn append_web_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        severity: &str,
        now_ts: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO web_messages (session_id, role, content, severity, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(role)
        .bind(content)
        .bind(severity)
        .bind(now_ts)
        .execute(&self.pool)
        .await?;
        self.touch_web_session(session_id, now_ts).await?;
        Ok(())
    }

    /// 该会话的全部消息, 按写入顺序(重开页面时原样重建对话流, FR-021)。
    pub async fn list_web_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<WebMessageRecord>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT message_id, session_id, role, content, severity, created_at
             FROM web_messages WHERE session_id = ? ORDER BY message_id",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(web_message_from_row).collect())
    }

    /// 该会话**最近 `rounds` 轮的问答消息**(FR-020): 只取 `user` / `assistant` 两种角色
    /// (宿主提示行不属于对话上下文), 起点 = 倒数第 `rounds` 条用户消息; 不足则返回全部。
    pub async fn recent_web_rounds(
        &self,
        session_id: &str,
        rounds: u32,
    ) -> Result<Vec<WebMessageRecord>, sqlx::Error> {
        if rounds == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT message_id, session_id, role, content, severity, created_at
             FROM web_messages
             WHERE session_id = ?1 AND role IN ('user', 'assistant')
               AND message_id >= COALESCE((
                     SELECT message_id FROM web_messages
                     WHERE session_id = ?1 AND role = 'user'
                     ORDER BY message_id DESC LIMIT 1 OFFSET ?2
                   ), 0)
             ORDER BY message_id",
        )
        .bind(session_id)
        .bind(i64::from(rounds) - 1)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(web_message_from_row).collect())
    }

    /// 某会话的消息行数(校验级联删除是否生效)。
    pub async fn web_message_count(&self, session_id: &str) -> Result<i64, sqlx::Error> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM web_messages WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("n"))
    }

    /// 把会话的最后活动时间推到 `now_ts`。
    async fn touch_web_session(&self, session_id: &str, now_ts: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE web_sessions SET updated_at = ? WHERE session_id = ?")
            .bind(now_ts)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
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

/// Web 会话消息的角色 (025 / FR-018): 用户输入 / 助手回复 / 宿主提示行(带级别)。
pub const WEB_ROLE_USER: &str = "user";
pub const WEB_ROLE_ASSISTANT: &str = "assistant";
pub const WEB_ROLE_HOST: &str = "host";

/// Web 会话记录 (025 / FR-018): 标题 + 创建时间 + 最后活动时间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSessionRecord {
    pub session_id: String,
    /// 空串 = 首条用户消息尚未到达(标题由首条用户消息截断生成, FR-022)。
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Web 会话内的一条消息 (025 / FR-018): 角色 + 内容 + 输出级别 + 时间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebMessageRecord {
    pub message_id: i64,
    pub session_id: String,
    /// [`WEB_ROLE_USER`] / [`WEB_ROLE_ASSISTANT`] / [`WEB_ROLE_HOST`]。
    pub role: String,
    pub content: String,
    /// 宿主显式标注的输出级别(FR-013 的 `Severity`, 原样落库)。
    pub severity: String,
    pub created_at: i64,
}

fn web_message_from_row(r: sqlx::sqlite::SqliteRow) -> WebMessageRecord {
    WebMessageRecord {
        message_id: r.get("message_id"),
        session_id: r.get("session_id"),
        role: r.get("role"),
        content: r.get("content"),
        severity: r.get("severity"),
        created_at: r.get("created_at"),
    }
}

/// 读取十进制文本列 (026): 落库一律存字符串, 与既有 fills / funding_fees 口径一致。
fn dec_of(r: &sqlx::sqlite::SqliteRow, col: &str) -> Decimal {
    Decimal::from_str(r.get::<String, _>(col).as_str()).unwrap_or_default()
}

fn order_record_from_row(r: &sqlx::sqlite::SqliteRow) -> OrderRecord {
    OrderRecord {
        strategy_id: r.get("strategy_id"),
        exchange_order_id: r.get("exchange_order_id"),
        client_order_id: r.get("client_order_id"),
        pair: r.get("pair"),
        side: r.get("side"),
        price: dec_of(r, "price"),
        size: dec_of(r, "size"),
        filled_size: dec_of(r, "filled_size"),
        status: r.get("status"),
        mode: r.get("mode"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
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

    #[tokio::test]
    async fn test_web_session_crud_cascade_and_title_once() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_web_session("s1", 100).await.unwrap();
        db.create_web_session("s2", 100).await.unwrap();
        // 最后活动相同 → 按 session_id 倒序
        assert_eq!(db.list_web_sessions().await.unwrap()[0].session_id, "s2");

        db.append_web_message("s1", WEB_ROLE_USER, "回测 BTC 30 天", "normal", 110).await.unwrap();
        db.append_web_message("s1", WEB_ROLE_HOST, "[用量] 1.2k tokens", "notice", 120)
            .await
            .unwrap();
        db.append_web_message("s1", WEB_ROLE_ASSISTANT, "好的", "normal", 130).await.unwrap();

        // 追加消息即把最后活动推后 → 升到列表最前
        let sessions = db.list_web_sessions().await.unwrap();
        assert_eq!(sessions[0].session_id, "s1");
        assert_eq!(sessions[0].updated_at, 130);

        // 标题只写一次(首次用户消息生成, 后续调用不覆盖)
        assert_eq!(db.set_web_title_if_empty("s1", "回测 BTC 30 天").await.unwrap(), 1);
        assert_eq!(db.set_web_title_if_empty("s1", "别的").await.unwrap(), 0);
        assert_eq!(db.list_web_sessions().await.unwrap()[0].title, "回测 BTC 30 天");

        // 消息按写入顺序返回, 级别原样保留
        let msgs = db.list_web_messages("s1").await.unwrap();
        let got: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(got, vec![WEB_ROLE_USER, WEB_ROLE_HOST, WEB_ROLE_ASSISTANT]);
        assert_eq!(msgs[1].severity, "notice");

        // 删除会话 → 消息外键级联删除, 不残留孤儿
        assert_eq!(db.web_message_count("s1").await.unwrap(), 3);
        assert_eq!(db.delete_web_session("s1").await.unwrap(), 1);
        assert_eq!(db.web_message_count("s1").await.unwrap(), 0);
        assert_eq!(db.delete_web_session("s1").await.unwrap(), 0, "重复删除应为 0 行");
        // 另一个会话不受影响
        let left = db.list_web_sessions().await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].session_id, "s2");
    }

    #[tokio::test]
    async fn test_web_recent_rounds_keeps_last_n_pairs_only() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_web_session("s1", 0).await.unwrap();
        for i in 0..30_i64 {
            db.append_web_message("s1", WEB_ROLE_USER, &format!("问{i}"), "normal", i)
                .await
                .unwrap();
            db.append_web_message("s1", WEB_ROLE_HOST, "提示", "notice", i).await.unwrap();
            db.append_web_message("s1", WEB_ROLE_ASSISTANT, &format!("答{i}"), "normal", i)
                .await
                .unwrap();
        }
        // 最近 20 轮 = 20 问 + 20 答; 宿主提示行不属于对话上下文
        let got = db.recent_web_rounds("s1", 20).await.unwrap();
        assert_eq!(got.len(), 40);
        assert_eq!(got[0].content, "问10");
        assert_eq!(got.last().unwrap().content, "答29");
        assert!(got.iter().all(|m| m.role != WEB_ROLE_HOST));
        // 轮数超过实际 → 返回全部; 0 轮 → 空
        assert_eq!(db.recent_web_rounds("s1", 99).await.unwrap().len(), 60);
        assert_eq!(db.recent_web_rounds("s1", 0).await.unwrap().len(), 0);
    }

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
        assert!(
            !db.insert_funding_fee("s2", &f).await.unwrap(),
            "跨策略同一流水同样去重(账户级唯一)"
        );
        assert_eq!(db.funding_total(None).await.unwrap(), (1, dec!(-0.12345678)));
        // 精确小数: 加分不再有 f64 尾差
        let f2 = FundingIncome {
            tran_id: "t2".into(),
            income: dec!(0.000004),
            time_ms: 2000,
            ..f.clone()
        };
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
