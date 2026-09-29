# 数据模型: 032 Web UI 工作台化

本功能不新增数据库表。所有持久化复用既有载体(`ricow.toml`、SQLite、`strategies/`、`run/`、`logs/`);仅回测作业为进程内内存结构。下列为接口层数据结构(serde 形态)。

## 1. 密钥配置(载体: ricow.toml)

来源/去向: `config_file::File` 的 `AiSection` / `ExchangeSection` / `MarketSection` / `UiSection`。

读响应 `GET /api/config/keys`(**绝不含全文**):

```json
{
  "binance_key":     { "configured": true,  "hint": "***a1b2" },
  "binance_secret":  { "configured": true,  "hint": "***9f0c" },
  "demo_key":        { "configured": false, "hint": null },
  "demo_secret":     { "configured": false, "hint": null },
  "ai": {
    "provider": "deepseek", "model": "...", "base_url": "...",
    "api_key": { "configured": true, "hint": "***c3d2" }
  },
  "market": { "show_all_pairs": false }
}
```

写请求 `POST /api/config/keys`:

```json
{ "updates": [ { "section": "exchange", "key": "binance_key", "value": "..." } ] }
```

校验规则: section∈{ai, exchange, market, ui};key 必须在 `config_file.rs` 的 AI_KEYS/EXCHANGE_KEYS/MarketSection 白名单内,否则 400;空字符串语义=清除该值(仅限非密钥字段;密钥字段空串=本次不修改)。

## 2. 交易对视野(载体: 既有 600s 缓存 + ricow.toml)

`GET /api/markets?all=0|1&q=STR&market=spot|futures`

```json
{ "spot": ["AAPLBUSDT", "..."], "futures": ["TSLAUSDT", "..."], "filtered": true }
```

规则: `all=1` 仅本次全量(不写盘);写盘式切换走 `POST /api/config/keys` 更新 `market.show_all_pairs`;`q` 大小写不敏感子串;`filtered` 取最终视野。

## 3. 行情明细

订单簿 `GET /api/markets/{symbol}/orderbook?market=spot|futures&depth=20`
→ `ricow_core::OrderBook` 直序列化:`{ bids:[{price,size}...], asks:[...], timestamp }`(bids 降序/asks 升序,已由 `new_sorted` 保证)。

K 线 `GET /api/markets/{symbol}/klines?market=&interval=1h&limit=200`
→ `ricow_core::Kline[]` 直序列化:`{ open_time, open, high, low, close, volume, close_time }[]`(Decimal 序列化为字符串,避免 JS 精度损失——既有 trades 端点同口径)。

校验: interval ∈ {1m,5m,15m,1h,4h,1d};limit 1~500(默认 200)。

## 4. 策略目录条目(沿用 031 catalog)

`GET /api/strategies` 现有 `StrategyRow` 扩展/新增"读源码"端点:

```json
{ "id": "...", "name": "...", "market": "spot|futures", "summary": "...",
  "source": "builtin|user", "params": [ { "key","name","type","desc","default","required","options" } ] }
```

`GET /api/strategies/{id}/source`(新):

```json
{ "id": "...", "market": "spot", "lua": "function on_tick...", "instance_toml": "仅用户策略: strategies/<id>.toml 原文;内置为 null" }
```

## 5. 策略保存请求

`POST /api/strategies`:

```json
{
  "name": "my-grid",
  "market": "spot",
  "pair": "ETHUSDT",
  "code": "function on_tick(ctx) ... end",
  "params": { "order_size": 0.01, "atr_mult": 2.0 },
  "overwrite": false
}
```

服务端状态转移与拒绝:
- 新名 → 名校验(`validate_strategy_name` 字符集/长度)→ 前缀冲突(`validate_new_name`)→ 保留名(`is_builtin_id`)→ 编译门禁(`create_strategy` 内 mlua 编译)→ 落盘
- 已存在 + `overwrite=false` → 409;`overwrite=true` 但实例在运行 → 409("先停止");否则备份旧文件(.bak)后覆盖,写 TOML 失败回滚 lua
- 编译失败 → 400 `{ "error": "...", "line": 12 }`(尽量带行号,取 mlua 错误原文)

## 6. AI 修改

`POST /api/strategies/{id}/ai-edit` 请求:`{ "instruction": "把网格间距改成 ATR 三倍" }`
成功:`{ "code": "<完整 lua>" }`(已过编译门禁);未配 AI key → 403 `{ "need_keys": true }`;超时/截断/编译失败 → 400 带原因。**不产生任何文件写入。**

## 7. 回测作业(内存)

```json
// POST /api/backtest
{ "strategy": "my-grid", "pair": "ETHUSDT", "interval": "1h",
  "days": 90, "market": "spot", "params": { "...": "..." } }
// → 202 { "job_id": "uuid" }
// GET /api/backtest/{job_id}
{ "status": "running|done|error", "report": "...", "error": null }
```

状态:`running → done|error`(单向)。并发:已有 running → 409。生命周期:done/error 保留 5 分钟后惰性删除,再查 404。

## 8. 策略实例运行态(来源: daemon 台账 + SQLite)

`GET /api/strategies/{name}/status`:

```json
{ "name": "my-grid", "running": true, "mode": "dry_run|demo|live|unknown",
  "pid": 12345, "uptime_secs": 3600, "pair": "ETHUSDT",
  "source": "daemon|ledger|none",
  "pnl": { "realized": "1.23", "fees": "0.10", "net": "1.13", "trade_count": 12, "asof": "..." } }
```

规则: source 三态如实(026 既有口径):daemon 在线 / daemon 不在线仅台账 / 无记录;pnl 取 `db.recent_pnl_snapshots(Some(name),1)`(Decimal 字符串)。

`POST /api/strategies/{name}/start`:

```json
{ "mode": "dry_run|demo|live", "pair": "ETHUSDT",
  "params": { "...": "..." }, "phrase": "确认实盘 my-grid" }
```

- dry_run/demo: 忽略 phrase;demo 缺凭据 → 400 `{ "need_keys": true }`
- live: phrase 必须严格等于 `确认实盘 <name>`(复用 `require_explicit_phrase` 校验器或同实现),不符 → 400 且无副作用;随后 `live_preflight` 检查 risk_ack → 未确认 → 403 `{ "need_risk_ack": true, "disclosure": "<风险披露全文>" }`;TOML 未声明 live_enabled → 400
- 成功:`{ "pid": 12345, "mode": "live" }`;已在运行 → 409

`POST /api/risk-ack`:`{ "phrase": "<披露页指定短语>" }` → 短语匹配后 `write_risk_ack()`;不匹配 400 且不落盘。

`POST /api/strategies/{name}/stop`:`{ "close_all": false }` → 复用 `stop_daemon`,返回停机报告;未运行 → 400 带 daemon 原文。

## 9. 错误格式(统一)

错误响应统一 `{ "error": "中文可读信息", "code": "need_keys|conflict|..." }`;HTTP 400 参数错 / 401 鉴权 / 403 需前置动作 / 404 不存在 / 409 冲突 / 500 内部错。
