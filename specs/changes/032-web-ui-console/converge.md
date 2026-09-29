# 032 收敛记录: Web UI 工作台化

**变更**: [spec.md](./spec.md) · [plan.md](./plan.md) · [tasks.md](./tasks.md) · [quickstart.md](./quickstart.md)

**日期**: 2026-09-29 · **状态**: 已收敛(实现 + 取证完成, 未提交)

**交付概览**: Web UI 由单一对话视图演进为**五视图工作台**(设置 / 市场 / 策略 / 运行 / 对话)——左侧导航 + hash 路由, 无构建 vanilla JS(common/settings/markets/strategies/runs/chat/router/app 8 个文件), lightweight-charts UMD 内嵌编译进二进制。后端新增配置/密钥统一端点、策略保存与源码/AI 编辑、回测 job、运行面(ensure_daemon + start/stop/status)与风险门禁接线; 对话视图与 CLI 零回归。

## 一、验收标准逐条结论(SC-001~007)

| SC | 结论 | 证据 |
|:--|:--|:--|
| SC-001 不碰终端 5 分钟配齐三类密钥 + AI 对话成功 | ✅ | 场景 1 浏览器实测: 设置页依次配置币安 API / demo / AI 密钥, 保存后 AI 对话正常应答(见 [quickstart.md](./quickstart.md) 执行记录) |
| SC-002 3 次点击看到 K 线; 默认视野与 CLI 过滤 100% 一致 | ✅ | 场景 2 走查(2026-09-29): 导航→市场(BTCUSDT)→详情 K 线渲染(lightweight-charts, 15m 切换重建); 默认 bStock 视野实测 78+168 与 CLI 一致, 搜索 AAPL 两组同步过滤, 「显示全部」1367+776 双向切换(写盘 `market.show_all_pairs` + 重取)往返一致(见 quickstart.md 场景 2 执行记录) |
| SC-003 复制内置→回测报告全程不离开浏览器 | ✅ | 策略视图: 复制内置→调参→`POST /api/backtest`(202+job_id)→轮询出报告; 用户亲手保存保留编译门禁(语法错误 400 带行号, 旧文件零破坏) |
| SC-004 live 启动 100% 双道逐字短语门禁; 错/空输拒绝率 100% | ✅ | T036 + 浏览器实测: 启动模态逐字输 `确认实盘 <策略id>` → 首次 live 服务端 403 need_risk_ack → 页面展开风险披露 → 逐字输 `确认风险` 落盘 `risk_ack.json` → 再提交通过; 错输/空输被拒且无确认残留(见 quickstart 场景 4) |
| SC-005 demo 生命周期页内闭环 + 日志可核对测试网域名 | ✅ | T036 真机: 运行视图启动 demo 实例 → 真实测试网成交 → 收益可见 → 停止; 行内日志面板(SSE)流入历史与实时行, 测试网域名请求可核对 |
| SC-006 对话与终端零回归 | ✅ | 场景 5 浏览器实测: 对话视图收发/术语解释/会话切换如常; `cargo test --workspace` 全绿(见 §二) |
| SC-007 接口响应与浏览器存储密钥全文检出 = 0 | ✅ | T040: 自动化扫描测试覆盖全部 9 个 js 资产(`web/mod.rs` test_frontend_stores_nothing_and_masks_secret_input)+ 10 路径×无/错 token 401 矩阵(空体断言); 密钥回显仅尾 4 位 |

## 二、门禁证据(T042)

```
cargo fmt --all -- --check          → exit 0 (0 差异)
cargo clippy --all-targets -- -D warnings → exit 0 (0 警告)
cargo test --workspace              → 589 passed / 0 failed / 22 ignored (exit 0)
  明细: 279+3+44+16+77+167+3 = 589 passed; ignored 1+9+3+3+1+2+2+1 = 22
```

较 031 基线 538: **+51 通过 / ignored 不变(22)**, 增量全部来自本变更(web 模块与 daemon 前置单测); roadmap 测试基线已更新(当前基线 2026-09-29, Windows)。

## 三、浏览器实测与 testnet 实证

- **场景走查**(2026-09-29, quickstart 执行记录): 场景 1(设置)/2(市场浏览+K 线)/3(策略)/5(对话零回归)全清单走查; 场景 4(运行)含 demo 生命周期与 live 拒绝路径; 期间修 3 处前端缺陷(`/lang` 词条、mk/sg id 前缀、CRLF 致 dirty 常亮及根因)。
- **runs 行内日志面板**: SSE 流入 178 行历史日志(含停机统计), 面板跨 5s 轮询刷新保持不断流; 停止后保存锁定解除→真实保存→`已保存`+dirty 灭, 全回路闭环。
- **testnet 纪律**: demo 实发下单真实成交; live 路径仅实证门禁拒绝链, 不发真实 live 单(与 019/025 同口径); 全程零 mock 交易替身。
- **i18n 全量核对**(T039): 脚本核对 8 个 js 的 zh/en 词典逐键一致(common 32/settings 29/markets 22/strategies 70/runs 51)、全部 `t()` 调用有词条、index.html data-zh/data-en 配对齐全; 四类错误态文案均有中文前缀。

## 四、治理同步(T041, FR-027~029)

- `specs/constitution.md` 升 **1.2.0**: 安全要求章确认渠道由 2 个增至 **3 个** —— 新增 Web 渠道(页面显式交互承载普通写操作确认; 用户亲手新建/编辑保留编译门禁, 免沙箱回测+preview+approve 链; **live 全部动作不降级**: 双道逐字短语缺一不可, daemon 侧门禁复用不变; 终端与对话渠道一行不改)。
- `specs/changes/025-web-ui/spec.md` 顶部加演进注记(冲突处以 032 为准)。
- `specs/architecture.md` §五 新增 032 段(五视图+hash 路由+D18 部分推翻声明、前端脚本拆分、**端点清单已按真实路由核准**: `GET/POST /api/config/keys`、`POST /api/strategies`、`POST /api/backtest`+`GET /api/backtest/{job_id}`、`GET /api/logs/{name}/stream` 等)。
- `specs/roadmap.md`: 测试基线 589 当前 + 032 注记块。

## 五、偏差与遗留(如实)

1. **fmt 差异在 T042 收口时发现**: 实施期写入的 `web/strategy_io.rs` 与 `ricow_engine/src/lib.rs` 与 rustfmt 输出不一致(fmt --check exit 1); 已 `cargo fmt --all` 对齐并复跑 clippy + 全量 test 全绿(纯格式, 无语义变化)。教训: 实施期每批改动应随手跑 fmt --check。
2. **真实 live 下单未实证**: 按 testnet 纪律只验证双道门禁与拒绝路径; live 真单随 P4 dogfood(需主网资金, 用户执行)。
3. **Windows 双击入口(T034/T035 类)**: 本机限制同 019, 不在本变更范围。
4. **一次性产物(已清理)**: `D:\tmp\ricow-032-e2e` 沙箱(RICOW_ROOT 测试现场 + shannon-test-1 产物 + i18n 核对脚本)已于 2026-09-29 删除, 后台 web 服务(端口 18432)已停; token 随进程终止轮换, 无残留。
5. **token 轮换**: web 一次性 token 每次重启重新生成, 无残留风险。

## 六、未提交

代码与文档改动均在工作区, 未 `git commit`(宪法提交纪律: 用户明确说"提交"才提交)。
