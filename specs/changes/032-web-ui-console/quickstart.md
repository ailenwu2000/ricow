# 快速验证手册: 032 Web UI 工作台化

## 前置

- Rust 工具链;构建:`cargo build`
- 测试网凭据记录见 `specs/testnet.md`(demo 现货/合约 key,禁主网下单测试)
- 本机代理(如需下载 vendored 图表库):1080 端口 http 代理
- 全新数据目录建议:`set RICOW_ROOT=<临时目录>`(Windows)后验证,避免污染日常数据

## 单元/静态验证

```text
cargo test              # 全量测试必须绿
cargo clippy --all-targets -- -D warnings
```

重点新增单测:
- web:新端点无 token/错 token → 401;GET keys 响应不含任何密钥全文;前端各 js 资源纳入"密钥不落浏览器存储"扫描
- 引擎:`write_strategy_files` 抽取后,`execute_strategy` 既有测试全绿;新名/覆盖/回滚/路径穿越
- 命名:保留名占用拒绝、前缀冲突拒绝(复用 `validate_new_name` 向量)
- backtest_jobs:状态转移、单并发拒绝、过期清理
- ensure_daemon:陈旧台账清理、就绪轮询(临时目录,不起真实交易)

## 端到端场景(人工 + testnet)

### 场景 1:密钥设置(US1)
1. `cargo run -- web --no-open`,浏览器开打印的 URL
2. 左侧点"设置":填入币安 key/secret、AI key(demo 留空)→ 保存成功
3. 检查 `%RICOW_ROOT%/ricow.toml` 字段更新、文件权限仅当前用户;刷新页面只见 `***xxxx`;DevTools 检查 localStorage/sessionStorage 无密钥
4. 去对话视图发一句普通问答,AI 正常应答(FR-009 全局生效)

### 场景 2:市场浏览 + K 线(US2,公开接口免 key)
1. 点"市场":默认仅 bStock 现货 + 美股永续;数量与 `cargo run -- pairs` 一致
2. 打开"显示全部"→ 列表变全量;重启 web 后仍为全量(配置持久化);`cargo run -- pairs` 同视野
3. 搜索 `AAPL` 两组同步过滤;点开 BTCUSDT:订单簿有买卖盘,K 线蜡烛图渲染;切 15m/4h/1d 图表刷新
4. 断网后点重试:出现错误卡片而非白屏

### 场景 3:策略管理 + AI + 回测(US3)
1. "策略"页复制 `shannon_spot_grid` → 新名 `e2e-grid-1` → 落盘;磁盘出现 `strategies/e2e-grid-1.toml` 与 `strategies/spot/e2e-grid-1.lua`
2. 改名尝试:`shannon_spot_grid`(保留名)与 `e2e-grid-1x`(前缀,构造冲突样例)均被拒
3. 编辑器故意写坏 Lua → 保存被拒,出现编译错误;文件未损坏
4. AI 修改:输入"把 order_size 减半"→ 返回 AI 草稿(未保存标记)→ 点保存落盘
5. 回测:标的 ETHUSDT、90 天、1h → 轮询后展示报告,指标与 `cargo run -- backtest --strategy e2e-grid-1 --pair ETHUSDT --days 90` 同口径
6. 启动该策略(Dry Run)后回编辑器点保存 → 409 提示先停止

### 场景 4:运行控制 + 实盘门禁(US4)
1. "运行"页启动 `e2e-grid-1` Dry Run → 状态变运行、日志滚动;停止 → 状态已停止
2. 填好 demo key 后以 demo 模式启动:`logs/e2e-grid-1.log` 中出现 `demo-api.binance.com`/`demo-fapi.binance.com` 真实请求;等待成交后页面收益/成交数非零;停止(可勾平仓)
3. live 门禁(不真实成交,只验拒绝路径):
   - 未确认风险时选 live 启动 → 403,页面展示披露全文
   - 错输风险短语 → 拒绝,`risk_ack.json` 不存在;输入 `确认风险` → ack 落盘
   - 启动 live 时错输 `确认实盘 e2e-grid-1` 以外内容 → 拒绝;TOML `live_enabled=false` 时即使短语正确也拒绝(与终端同口径)
4. 杀掉 daemon 进程后在页面启动 → 自动拉起成功;制造端口占用/日志不可写时页面出现可读错误

### 场景 5:对话零回归(US5)
1. `#chat` 视图:新建会话、流式回复、`/help`、`/lang`→EN 后全视图英文、三个面板数据正常
2. 在各视图间切换后刷新 → 停留在刷新前视图

## 收敛检查(阶段 5)

- 宪法"安全要求"章已增 Web 渠道条款;025 spec 顶部已加修订注记
- architecture.md Web 章节与 roadmap.md 测试基线数字已更新
- 临时验证目录清理;`git status` 仅含预期变更

## 执行记录(2026-09-28,场景 4 / T036)

环境:`RICOW_ROOT=D:\tmp\ricow-032-e2e`(拷贝含 demo 凭据的 ricow.toml),`ricow web --port 18432`;策略经 `POST /api/strategies` 从 `shannon_spot_grid` 复制为 `e2e-grid-1`(spot/ETHUSDT,编译门禁+双文件落盘)。全部走 API 实测(页面同源)。

- **live 拒绝路径(全过,零副作用)**:无/错短语 → 400 回显「确认实盘 e2e-grid-1」;对短语未确认 → 403 `need_risk_ack` 含披露全文,`risk_ack.json` 未落;`POST /api/risk-ack` 错短语 → 400 不落盘,「确认风险」→ 200 `{acked:true}` 落盘(version=1);确认后 live 因 TOML `live_enabled=false` → 400 `live_disabled`;daemon 未被自举。
- **demo 实发(testnet 真实调用)**:`端点: https://demo-api.binance.com`、时钟预检通过(+459 ms)、账户快照 4826.65 USDT、预装 K 线(1h×24 + 1m×1000)、user data stream(WS-API)就绪;激活后市价单经交易所精度对齐(step_size 0.0001)→ **真实成交 0.0074 ETH @ 2686.20(19.88 USDT)**,`[FILL] #1` 与权益更新;`GET /api/runs` trade_count=1、fees/net 非零;网格 built=true 正常运转。停止 → 测试网撤单兜底,持仓保留。
- **daemon 自举/拉起**:dry_run 启动自动自举 daemon;`taskkill /F /T` 杀 daemon 树后 `GET /api/runs` 如实回退 `source=ledger`(running:false),再次 start 幂等自举新 daemon 并成功拉起。
- **日志不可写**:`logs/<name>.log` 置为目录后启动 → 400 中文可读错误「启动失败: 拒绝访问。 (os error 5)」。
- 备注:初次 demo 启动把 `atr_interval` 设为 1m,触发策略自身 R7 成本门槛(2×ATR 0.17% < 4×费率 0.4%)自停——策略保护行为正确;改回默认 1h 后通过并成交。

## 执行记录(2026-09-29,场景 1/3/5 浏览器实测 + T029/T037/T038)

同一环境(RICOW_ROOT=D:\tmp\ricow-032-e2e,端口 18432),全流程在真实浏览器中操作(无 mock)。期间发现并修复 3 个前端缺陷(重建+重启后复验):

- **修复 1 `/lang` 回归**:切英文后 `chat.js` 调用不存在的 `t("langOk")` 报错 → 补词条。
- **修复 2 id 冲突**:markets.js 与 strategies.js 重复 id(`mk-*`/`sg-*` 前缀统一)导致 getElementById 错串 → 改前缀。
- **修复 3 CRLF dirty 常亮**:策略详情进页即显示「有未保存的修改」且撤销不灭。根因 = textarea value getter 把服务端 CRLF 原文规范化为 LF,而 `savedCode` 存了原文,二者永不相等(strategies.js L720-721:改为先赋值再取 getter 规范化后的值)。复验:CRLF 策略进详情 dirty=false,编辑→撤销回路 dirty 正确灭。

**场景 3(US3,全部通过)**:复制模态预填(`shannon_spot_grid-1`/BTCUSDT);保留名 `shannon_spot_grid` → 拒绝「内置策略保留标识…(FR-016)」;互前缀 `e2e-grid` → 拒绝并给撤单归属解释;副本 `shannon-test-1` 创建成功(toml+lua+备份落盘,详情打开);坏 Lua 注入 → 「编译未通过: 第 1 行: …」且原文件不损坏;撤销/保存回路正常(保存 → 「已保存」+ 落盘更新 + 自动 .bak);AI 修改错误路径 402 Insufficient Balance 完整呈现、无假草稿(**deepseek 余额不足,成功流式为已知外部限制**);回测 days=7/1h 约 6 秒完成(168 根 K 线、指标齐全);运行中保存 → 409「正在运行…(FR-020)」双保存钮锁定,停止后按钮恢复且真实保存通过(锁定解除回路)。

**场景 4 补充(runs 视图)**:表格渲染/5s 自动刷新;启动模态(默认 dry_run,live 需逐字短语);dry_run 启动 → 行内状态「运行中」;停止确认模态(close_all)→ 报告「exit=0, 用时 100ms」+ on_stop 清理提示;行内日志面板(SSE)打开后流入 178 行历史日志(含停机统计),面板跨轮询刷新保持。

**场景 5(US5,零回归)**:对话视图会话增删切、流式着色、/help /history /lang /keys 均正常;视图切换(#chat/#markets/#strategies/#runs/#settings)数据随 activate 刷新;对话 SSE 跨视图保持;刷新后 hash 恢复;市场「显示全部」勾选双向切换(现货 78 对 ↔ 全部含杠杆对,写盘+重取)。

## 执行记录(2026-09-29,场景 2 浏览器实测 / T018 补验)

同一环境(端口 18432),浏览器实测市场列表与详情(公开接口全程免 key):

- **默认视野**:进入市场页默认过滤视野,标注「过滤视野 · 现货 78 · 合约 168」,现货组全部为 `*BUSDT`(bStock 美股现货)、合约组为 `*USDT`(美股合约),与 `ricow pairs` 视野一致。
- **显示全部开关**:勾选后标注变「全部视野 · 现货 1367 · 合约 776」全量列表(含 0GUSDC/1000CAT 等加密与杠杆对),取消后恢复 78/168,双向切换正常(与场景 5 记录互证)。
- **搜索框**:输入 `AAPL` → 现货/合约两组同步过滤为「现货 1(AAPLBUSDT) · 合约 1(AAPLUSDT)」;输入 `BTC`(全部视野)→ 现货 53 · 合约 7。
- **详情页(BTCUSDT 现货)**:hash 切 symbol 数据全部刷新;现价「买卖盘中间价 82970.855」= 买一 82970.85 与卖一 82970.86 的中点(depth=1 mid 取价正确);买/卖盘各渲染 20 档价格数量;K 线区 LightweightCharts 蜡烛图画布渲染(TradingView attribution 出现);周期按钮 15m/1h/4h/1d,切 15m 后显示「加载中…」→ 加载完成图表重建。
- **断网错误态未实测**:浏览器环境断网难以真实模拟;markets.js 的失败卡片+重试词条以代码实现与审查为准(加载失败不展示伪数据)。

测试产物:`shannon-test-1`(toml/lua/.bak,含 e2e 探针注释)留在 `D:\tmp\ricow-032-e2e`,收敛后随临时目录清理。
