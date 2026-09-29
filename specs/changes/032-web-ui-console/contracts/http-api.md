# HTTP 接口契约: 032 Web UI 工作台化

**通用约束**:
- Base: `http://127.0.0.1:{port}`;全部端点(含静态资源)在既有 Bearer token 中间件之后
- 鉴权: 请求头 `Authorization: Bearer <token>`;静态资源与 EventSource 走 `?token=`
- 错误体统一: `{ "error": "中文信息", "code": "<机器码可选>" }`
- Decimal 一律 JSON 字符串(防 JS Number 精度损失)
- 写操作幂等/冲突语义见各端点;所有写操作记录不另建审计表(本地单用户工具)

## 静态资源

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/` | index.html(token 占位符服务端替换,既有机制) |
| GET | `/common.js` `/router.js` `/chat.js` `/settings.js` `/markets.js` `/strategies.js` `/runs.js` `/app.js` | 拆分后前端资源,`include_str!` 嵌入 |
| GET | `/lightweight-charts.js` | vendored UMD,Content-Type: text/javascript |
| GET | `/style.css` | 既有,扩充新视图样式 |

## 1. 密钥配置

### GET /api/config/keys
200: 见 data-model §1。永不返回密钥全文;hint = `***` + 末 4 字符(不足 4 位则 `***`)。

### POST /api/config/keys
Body: `{ "updates": [{ "section": "ai|exchange|market|ui", "key": "...", "value": "..." }] }`
- key 白名单校验(越权字段 400);逐条经 `config_file::set_values` 落盘
- 密钥字段 value="" 视为不修改;非密钥字段 "" 视为清空
- 204 成功;配置文件不可写 500 且不产生部分写入失败时的脏内容(set_values 原子语义,实现时验证)

## 2. 市场浏览

### GET /api/markets
Query: `all=0|1`(默认 0,1=本次全量不写盘)、`q=<子串>`、`market=spot|futures`(可选)
200: `{ spot: string[], futures: string[], filtered: boolean }`
实现: `pairs::current_view(root, all==1)` → `filter_view(view, market, q)`。

### GET /api/markets/{symbol}/orderbook
Query: `market=spot|futures`(必填)、`depth=1..50`(默认 20)
200: OrderBook JSON;400 market 缺失/非法;502 上游失败(可读错误)。

### GET /api/markets/{symbol}/klines
Query: `market`(必填)、`interval∈{1m,5m,15m,1h,4h,1d}`(默认 1h)、`limit=1..500`(默认 200)
200: Kline[];时间字段 ISO8601 字符串。

(24h ticker 为可选增强;首期不做,详情页用 depth=1 订单簿的 mid_price 当现价。)

## 3. 策略管理

### GET /api/strategies / GET /api/strategies/{id}
既有端点保留(031),响应补 `source` 等已有字段,不破坏现有前端消费。

### GET /api/strategies/{id}/source
200: `{ id, market, lua, instance_toml }`;内置策略 instance_toml=null;404 未知 id。

### POST /api/strategies(新建/复制/覆盖保存)
Body: 见 data-model §5。
- 200 `{ name, toml_path, lua_path, backup? }`
- 400 名字非法 / market 非法 / 编译失败(`{error, code:"compile", line?}`)
- 409 同名存在且未 overwrite;或与内置保留名/前缀冲突(code:`reserved`/`prefix`);或实例运行中(code:`running`)

### POST /api/strategies/{id}/ai-edit
Body: `{ instruction: string(非空) }`
200 `{ code }`(已过编译门禁,未落盘);403 未配 AI key(`{code:"need_keys"}`);400 LLM 失败/超时/返回代码编译失败。

## 4. 回测

### POST /api/backtest
Body: 见 data-model §7。202 `{ job_id }`;已有运行中作业 → 409(code:`busy`)。
执行: tokio spawn_blocking 跑回测内核(复用 CLI 同一 K 线拉取与撮合),报告唯一来源 `format_backtest_report`。

### GET /api/backtest/{job_id}
200 `{ status, report?, error? }`;404 作业不存在或已过期。

## 5. 策略运行

### GET /api/runs
200: 全部实例视图数组(来源 `instances::snapshot`),每项 `{name, running, mode, pid, uptime_secs, pair, source}`(列表用,不含 pnl)。

### GET /api/strategies/{name}/status
见 data-model §8(含 pnl 最近快照);source 三态如实。

### POST /api/strategies/{name}/start
Body: `{ mode, pair?, params?, phrase? }`
- dry_run: 直接启动;demo: 缺 demo 凭据 400 need_keys
- live: phrase 必须逐字等于 `确认实盘 {name}`(常量时间比较与终端同实现);不符 400;未读风险披露 403 `{code:"need_risk_ack", disclosure: RISK_DISCLOSURE}`;live_enabled 未声明 400
- daemon 未运行: 内部 `ensure_daemon` 后再启动;拉起失败 500 带 daemon.log 指引
- 已在运行 409;成功 200 `{ pid, mode }`

### POST /api/risk-ack
Body: `{ phrase: string }`;phrase 逐字等于 `确认风险` → `write_risk_ack()` 落盘 204;否则 400 无副作用。
披露全文来源 `ricow_engine::RISK_DISCLOSURE`(GET 可不另开端点,start 的 403 体内带回)。

### POST /api/strategies/{name}/stop
Body: `{ close_all: bool }`;200 `{ report }`(daemon 停机原文);未运行 400。

### 日志
沿用既有 `GET /api/logs`、`/api/logs/{name}/tail`、`/api/logs/{name}/stream`(SSE),不新增。

## 6. 既有端点零回归
`/api/sessions*`、`/api/lang`、`/api/terms`、`/api/trades/*`、`/api/logs/*`、`/api/ping` 路径与响应不变。
