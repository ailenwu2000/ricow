# 062 收敛记录：fmz 风格 Web UI 重做

| 项 | 内容 |
| -- | -- |
| 编号 | 062 |
| 名称 | fmz 风格 Web UI 重做 |
| 状态 | 已实施并真机验证（2026-10-23） |
| 基准 | fmz.com 实站实登录勘察（账号 `sparkphatom`，账户 ID 144072307，2026-10-23） |
| 计划 | [`plan.md`](./plan.md) · [`tasks.md`](./tasks.md) · [`spec.md`](./spec.md) |

## 1. 三个拍板项的落地

| # | 决定 | 落地方式 |
| -- | -- | -- |
| Q1 | **默认主题由 `dark` 改 `light`**(fmz 浅色) | 四处一起改: `defaults::UI_THEME` / `SUPPORTED_THEMES` 首项 / `web::settings::get_theme` 缺省 / 前端 `common.js` 的 `R.theme` 初值与 `applyTheme` 兜底。**加断言** `test_default_theme_is_light_and_consistent` + `test_default_theme_matches_backend` 钉住。 |
| Q2 | **顶栏不留 fmz 主菜单占位** | 那一排全是社区/内容类(策略/围观/文库/社区/API文档/APP), ricow 没有 → 顶栏右端换成 ricow 自己的主题/语言/帮助/命令面板。 |
| Q3 | **涨跌色拆分** | `--accent` 只做主色(绿 `#1CAF83`); `--up`=涨(红 `#F5222D`)/`--down`=跌(绿 `#0D9B6C`, 比主色暗一档)。引用点全改: `.mk-book-col.is-ask` / `.sg-mcard-value.down` / `.sg-hist-table td.down` / `.sg-val-mval.down` + `markets.js` 跌色 + `strategy_backtest.js` 卖点色。 |

## 2. 做了什么

### 2.1 令牌层(P0)
- `:root` 与 `[data-theme="light"]` **写成一条规则**(= fmz 浅色, 默认档, 首屏不闪) → 浅色变量**只维护一份**; `[data-theme="dark"]` / `[data-theme="red"]` 各自覆盖。
- 新增尺寸令牌 `--radius` / `--radius-lg` / `--control-h` / `--topbar-h` / `--sider-w` / `--menu-item-h`; 文字层级 `--fg-2/3/4`; 边框 `--border-strong`; 代码高亮 `--hl-str/num/kw`; 阴影 `--shadow-pop/panel/modal`。
- **变量块外零写死颜色**(grep 验证)。原先散在 30+ 处规则里的写死值(圆角 12/10/8/6px、阴影 rgba)收进令牌。

### 2.2 骨架(P1)
- 三段式: 顶栏(全宽 64px, 最上) → `#shell`(侧栏 160px | 内容区, 同一行) → 内容区底 `#F4F6F7`。
  **中间那层 `#shell` 是必需的** —— 少了它, 侧栏与内容区会**上下排列**(内容区被挤到侧栏下面)。第一版就栽在这, 由 CDP 坐标断言抓出。
- 侧栏只放导航; **会话列表搬进 `#view-chat`**(`#session-list` / `#new-session` 的 **id 不变**, chat.js 一行没改)。
- 导航项加图标位(自绘内联 SVG, 见 `ui.js`); 导航文案包一层 `.nav-text`。
- 面包屑**由 router 统一写**(`renderCrumbs`), 不各视图各画一份。

### 2.3 通用组件层(P2)—— 新增 `assets/ui.js`
`R.ui.card / tabs / empty / breadcrumb / button / tag / stat / quick / icon / decorateNav`。
只产 DOM 节点(不拼 HTML 字符串)、不引入浏览器存储、不含业务逻辑。

### 2.4 概览视图(P4)—— 新增 `assets/dashboard.js` + `GET /api/overview`
fmz 的 P1 仪表盘范式: 统计卡(策略/实例/密钥/会话) + 快捷入口六宫格 + 开发文档卡 + 运行状态卡。
**新增后端聚合端点** `GET /api/overview`(见 §4 的坑②)。

### 2.5 换肤(P3/P5)
全站卡片/按钮/表格统一到 fmz 规范(圆角 4px、描边 `#F0F0F0`、表头 `#FBFBFB`+字重 600、按钮 32px 三档)。
用**覆盖层**收口(style.css 末尾), 不动 30+ 处既有规则 —— 改错了删掉那段就回到原状。

## 3. 刻意不做的事(与 spec 一致)

- **账号体系整块不做**: 登录/注册/账号设置/二次验证/额度预警/授权管理/推广返佣/账单/工单/私信/消息中心/APP/社区内容 —— 不留占位。
- **不引 React/antd/npm**: CSP `script-src 'self'` + 零依赖原生 JS + `include_str!` 嵌入 → fmz 的样子**手写实现**。
- **不做像素级 1:1 抄**: 抄信息架构 + 视觉规范 + 交互形态, 内容填 ricow 的真实数据。
- **fmz 的右端"账户卡"与"消息中心卡"不做** —— ricow 无账号; 换成「运行状态」卡。
- **全局搜索框只做外观**: 输入框 `disabled` + title 明说"尚未开放", **不做成假功能**。

## 4. 撞出来的三个坑(都记进了项目记忆)

### ① 浏览器 profile 里的**死连接**吃掉连接池
现象: 概览四格一起显示「—」, 而 `curl` 打同一端点只要 10ms; CDP 里连 `requestWillBeSent` 都没有 —— fetch 卡在"发送前"。
根因: 浏览器同源 HTTP/1.1 连接数上限 6, 本页常驻占着会话 SSE + 每 5 秒三路轮询; 而我反复重启服务却**复用同一个浏览器 profile**, 每次重启留下一条**死掉的 SSE 连接**, 越测越堵。
**判据**: `curl` 正常 + CDP 里连请求都没发出 → 是**浏览器侧连接池**, 先换新浏览器验证, 别去查后端。
**修法**: 每次换端口/重启服务后用**全新 `--user-data-dir`** 再测。

### ② `R.navigate` 只写 `location.hash` 在**首屏**失效
`location.hash = "#x"` 在首屏导航尚未提交时, Chromium 把它并入初始 URL、**不触发 hashchange**(实测监听器一次没被叫到)。后果: 第一次用编程式跳转"点了没反应"。
**修法**: `R.navigate` 写完 hash **自己再调一次 `applyRoute()`**(`applyRoute` 有同名同参早退守卫, 不会重复激活)。加回归断言 `test_navigate_switches_view_without_relying_on_hashchange`。

### ③ 依赖**精确空白**的 Rust 断言在 Windows 上长期是红的
`core.autocrlf=true` → 磁盘 CRLF、Rust 字面量 LF → 永远匹配不上。
基线里 `test_onboard_bar_is_driven_by_server_facts_only` **本来就是红的**(一直没人注意)。
**修法**: 断言改成**按空白切段、逐段顺序匹配**, 换 CRLF/LF 都过。

另有一处**顺带修掉的既有 bug**: `#view-title` 的 `color: var(--text)` —— `--text` **从来没定义过**, 该声明整体作废, 顶栏视图名一直继承父级颜色。

## 5. 验证

### 5.1 门禁六件
| 门禁 | 结果 |
| -- | -- |
| `cargo fmt --all -- --check` | **0 差异** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0** |
| `cargo test --workspace --no-fail-fast` | **1055 passed / 0 failed / 22 ignored** |
| `bash scripts/ci_grep_gates.sh` | **五条安全红线全绿** |
| `node scripts/web_frontend_check.mjs` | **全绿**(A 段 18 份 .js / 双语 **607·607** / 541 个引用键零缺零死) |
| 变异验证 | 见 §5.4 |

### 5.2 真机两路(与基线逐条对比)
| 通道 | 062 | 基线 | 差异 |
| -- | -- | -- | -- |
| e2e `e2e_web.py` | 375 PASS / 166 FAIL | 375 PASS / 166 FAIL | **逐条断言名完全一致**(`comm` 对比双向为空) |
| CDP `probe_web_ui.mjs` | 145 PASS / 11 FAIL | 145 PASS / 11 FAIL | **逐条一致** |
| 062 自带走查 `probe062.mjs` | **36 PASS / 0 FAIL** | — | 新增 |

- e2e 的 166 条与 CDP 的 11 条都是**既有的、与 062 无关**的失败: e2e 脚本期望内置策略 `shannon_grid` / `paired_grid`, 而当前内置集已改成 `paired_grid-12` 且现货 `shannon_grid` 已不在集里 —— **换内置策略时没同步 e2e 脚本**。CDP 的 11 条同为 `spacing_pct`(属 `paired_grid`)相关。
- 062 唯一影响的一条是 `主题缺省 dark → light`(Q1 的预期变更) —— e2e 脚本已同步(权威源在 skill 目录, 执行副本在 `tmp/`, 两份已同步)。

### 5.3 062 自带走查的判据(全部取 fmz 实测值)
顶栏高 **64** / 侧栏宽 **160** / 内容区左边距 **160** / 侧栏在顶栏之下 / 底色 `#F4F6F7` / 主色 `#1CAF83` / 面板白底 /
导航选中态 **绿底绿字**(`#E8FCF7` + `#1CAF83`) / 导航项高 **48** / 导航图标 7 个注入 / 概览 4 卡 + 5 统计 + 6 快捷 + 面包屑 /
**真实鼠标点击**导航能切视图且高亮跟随 / 会话列表在对话视图可见、切走即隐藏 / 七个视图都能进且有内容 /
深色与红色主题变量**零缺失** / 主色≠跌色、涨≠跌。

### 5.4 变异验证(证明断言非恒真)
新增的每条断言都做过变异: 把 `R.navigate` 里的 `applyRoute()` 去掉 → `test_navigate_switches_view_without_relying_on_hashchange` 转红;
把 `.nav-text` 包层去掉 → `test_nav_labels_wrapped_so_apply_lang_keeps_icons` 转红; 把 `SUPPORTED_THEMES` 首项改回 `dark` → 两条主题断言转红;
把 `/api/overview` 的 `Option` 全填 0 → `test_overview_returns_all_four_counts` 转红。

## 6. 基线更新(两处镜像)
`specs/roadmap.md` + `specs/architecture.md` 第九节: 基线改为 **1055 / 0 / 22**, 并订正了
「上一基线记的 `ricow_strategy` 290 / `ricow` bin 550 与实测不符(实为 277 / 553)」——
**基线一律整仓实跑读出, 分项只作叙述且要当场验加总**(历史上第二次栽在同一处)。

## 7. 遗留 / 建议后续

1. ~~**e2e 脚本与内置策略集脱节**(166 条)与 **CDP 的 `spacing_pct` 11 条**~~ —— 属 060/061 换内置策略时没同步探针, 当时记作"不在 062 范围内, 建议单独起一轮"。**该轮已做(063 深度检查)**: 把探针换成**真实存在的内置策略 id + 真实参数名** 后, 两路**全绿**(e2e 460/0、CDP 156/0)。详见 `specs/changes/063-fmz-aligned-strategy-live/converge.md` §1.5 —— 那批"既有失败"**掩盖掉了两个真产品 bug**, 教训一并记在那儿。
2. **各视图的深色主题下未逐项目视校对** —— 令牌层三处齐全(CDP 已验"零缺失"), 但卡片/表格在深底上的具体观感没逐页看。
3. **fmz 的四种页面范式**里, P3(卡片墙)与 P4(工具条+标签页)只在策略页部分落地; 若要把"策略编辑器"完全改成 fmz 的 P4 形态(工具条 + 代码/笔记/描述/手册/参数 五个标签页), 需另起一轮。**部分进展**: 063 已把策略编辑器改成**双 Tab**(「策略编辑」/「模拟回测」)+ AI 助手右置常驻, 但**离 fmz 的五个标签页还远**(笔记/描述/手册三块仍未做)。
4. **全局搜索框**仍是外观占位 —— 接真实搜索要定"搜什么"(策略/会话/术语)并做后端接口。
