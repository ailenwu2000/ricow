# 062：以 fmz.com（发明者量化）为基准重做 Ricow Web UI 视觉与信息架构

| 项 | 内容 |
| -- | -- |
| 编号 | 062 |
| 名称 | fmz 风格 Web UI 重做 |
| 类型 | 视觉与信息架构重构（UI 换皮 + 布局重组，非新功能） |
| 状态 | 计划已制定，待用户过目后开工 |
| 来源 | 用户要求「严格按照 fmz.com 的页面生成一套新的 UI」 |
| 基准 | fmz.com 实站实登录勘察（账号 `sparkphatom`，账户 ID 144072307，2026-10-23） |

## 1. 勘察方法与证据

不是"照着印象画"，是**登录 fmz.com 实地量出来的**：

1. **CDP 实机勘察**：headless Edge（`--headless=new`，CDP 端口 9444）驱动，`Runtime.evaluate` 取 `getComputedStyle` 的精确值、`getBoundingClientRect` 的精确坐标，`Page.captureScreenshot` 出图。
2. **登录**：fmz 登录页是 Remix + Ant Design 的受控表单，脚本注入 setter 会被 React 拒（`Illegal invocation`），最终由用户在可见窗口手动登录后继续。
3. **覆盖面**：抓取 18 个页面（`/m/dashboard`、`/m/strategies`、`/m/prop-strategies`、`/m/robots`、`/m/nodes`、`/m/platforms`、`/m/trade`、`/m/database`、`/m/billing`、`/m/tickets`、`/m/msg`、`/m/dm`、`/m/account`、`/m/add-strategy`、`/m/add-robot`、`/m/add-node`、`/m/authorization`、`/m/affiliate`）。
4. **证据落盘**：`tmp/fmz_browser/fmz_pages.json`、`fmz_pages2.json`、`shots/m-*.png`（18 张截图）、`tmp/fmz_browser/cdp.mjs`（通用 CDP 驱动，可复跑）。

> 注：`/m/robots` 渲染为空（该账号无实盘）、`/m/database` 与 `/m/messages` 停留在 loading，故这三页的结构以 DOM 探针与同构页（`/m/nodes`、`/m/msg`）为准，**不以截图为准**。

## 2. 从 fmz.com 量到的设计语言（权威值，非猜测）

### 2.1 色板

| 角色 | fmz 实测值 | 用途 |
| -- | -- | -- |
| 主色 | `rgb(28, 175, 131)` = **#1CAF83** | 选中菜单、主按钮、图标强调 |
| 主色浅底 | `rgb(232, 252, 247)` = **#E8FCF7** | 选中菜单项背景、提示条底色 |
| 页面底色 | `rgb(244, 246, 247)` = **#F4F6F7** | 内容区背景 |
| 卡片底色 | `rgb(255, 255, 255)` = **#FFFFFF** | 卡片、表头 |
| 卡片描边 | `rgb(240, 240, 240)` = **#F0F0F0** | 1px 边框 |
| 正文色 | `rgba(51, 51, 51, 0.88)` = **#333**（88% 不透明） | 主文本 |
| 次级文字 | `rgba(51, 51, 51, 0.65)` | 标签页未选中、按钮文字 |
| 三级文字 | `rgba(51, 51, 51, 0.45)` | 面包屑、辅助说明 |
| 占位文字 | `rgba(51, 51, 51, 0.25)` | placeholder |
| 分割线 | `rgb(240, 240, 240)` / `rgb(251, 251, 251)` | 表头底、行分隔 |

### 2.2 尺寸与形状

| 元素 | 实测值 |
| -- | -- |
| 顶栏高 | **64px**，白底，`padding: 0 24px 0 0` |
| 侧栏宽 | **160px**，白底（**不是深色**，尽管类名带 `ant-layout-sider-dark`） |
| 侧栏菜单项高 | **48px**，`padding: 0 16px 0 24px` |
| 侧栏选中态 | 底 `#E8FCF7` + 字 `#1CAF83`，**无左边框、无圆角** |
| 内容区内边距 | 左右各 **16px**（面包屑 x=176 = 160 + 16） |
| 面包屑 | 高 22px，文字三级色，格式「控制中心 / 策略库」 |
| 卡片圆角 | **4px**，白底 + `#F0F0F0` 边框，**无阴影** |
| 卡片 body padding | **16px 24px** |
| 表格头底色 | `#FBFBFB`，字重 **600**，`padding: 12px 16px`，高 50px |
| 表格单元格 padding | `0 16px` |
| 标签页 | 高 **50px**，选中项为主色下划线 |
| 按钮高 | **32px**，`padding: 0 15px`，圆角 **4px** |
| 主按钮 | 实色 `#1CAF83` 底 + 白字 |
| 次按钮 | **描边** `#1CAF83` 1px + `#1CAF83` 字 + 白底 |
| 文字按钮 | 无边框无底，主色字 |
| 表单控件 | 高 32px，圆角 4px |
| 间距单位 | 4px（antd `--ant-size-unit`） |
| 字号 | 基准 **14px** |

### 2.3 布局骨架（三段式）

```
┌────────────────────────────────────────────────────────┐
│ 顶栏 64px  [Logo] [搜索框] [菜单...]      [图标区][头像] │  白底
├──────────┬─────────────────────────────────────────────┤
│ 侧栏      │ 面包屑「控制中心 / 策略库」                  │  底 #F4F6F7
│ 160px     │ ┌─ 卡片: 标题条 + 操作按钮组 ─────────────┐  │
│ 菜单项     │ │  统计网格 / 快捷入口 / 表格 / 标签页      │  │
│ 48px      │ └──────────────────────────────────────────┘  │
│           │ ┌─ 卡片 ──────────────┐ ┌─ 卡片 ───────────┐  │  ← 栅格分栏
│           │ └─────────────────────┘ └──────────────────┘  │
└──────────┴─────────────────────────────────────────────┘
```

### 2.4 fmz 的四类页面范式

| 范式 | 特征 | 典型页 |
| -- | -- | -- |
| **P1 仪表盘** | 面包屑下多张卡片：统计数字网格（图标+数字+名称+箭头）、快捷入口方块网格、文档链接卡、右侧账户卡 | 控制中心 |
| **P2 表格页** | 卡片头放操作按钮组 → 标签页（带计数）→ 筛选行 → 表格 → 分页器 | 策略库、消息中心 |
| **P3 卡片墙** | 顶部一个分组标题条，下方多列卡片，每卡有标题/描述/标签/操作按钮 | 自营策略 |
| **P4 表单页** | 面包屑下工具条（语言下拉+名称输入+类型下拉+按钮组）→ 标签页（代码/笔记/描述/手册/参数设置）→ 大编辑区 | 新建策略、创建实盘 |

## 3. 需求：页面映射（**不含账号体系**）

> 用户明确：**Ricow Web 没有账号体系**，故 fmz 的 `登录 / 注册 / 账号设置 / 授权管理 / 推广返佣 / 账单 / 私信 / 消息中心 / APP下载` 一律**不做**，也不留占位。

### 3.1 侧栏导航（Ricow 版）

fmz 侧栏 15 项 → Ricow 取 6 项（与现有六视图一一对应，**不改路由语义**）：

| fmz 侧栏 | Ricow 对应 | Ricow 路由 | 说明 |
| -- | -- | -- | -- |
| 控制中心 | **概览**（新建） | `#dashboard` | 新增 P1 仪表盘范式 |
| 实盘 | 运行 | `#runs` | 表格范式对齐 |
| 策略库 | 策略 | `#strategies` | 表格范式对齐 |
| 托管者 | — | — | ricow 无托管者概念，不做 |
| 交易所 | 密钥 | `#keys` | 表格/卡片范式对齐 |
| 交易终端 | 市场 | `#markets` | 行情 + 订单簿 + K 线 |
| 数据探索 | — | — | 与"市场"重合，不做 |
| 账单/工单/消息/私信/账号/授权/返佣 | — | — | **账号体系相关，全部不做** |

fmz 侧栏另有「自营策略」（内置模板市场）→ Ricow 有**内置示例策略**（`catalog::run_block` 的内置项），归入「策略」视图的标签页，不单开一级菜单。

### 3.2 页面逐项映射

| Ricow 页面 | 源数据 | 目标范式 | 与 fmz 的差异 |
| -- | -- | -- | -- |
| **#dashboard**（新） | 各视图已有 API 聚合 | P1 | fmz 概览卡 → Ricow 改为：策略数 / 运行实例数 / 密钥数 / 最近对话数 / 快捷入口六宫格 / 快捷文档卡 |
| **#markets** | `/api/markets`、SSE | P2 + 图表 | fmz 无对应页（"数据探索"是数据仓库语义）→ Ricow 用「列表 + 详情」两段，配 P4 的工具条 |
| **#strategies** | `/api/strategies` | P2 表格 + P3 卡片墙 | 保留 Ricow 现有全部功能（新建/复制/编辑/回测/参数），只换外观与布局 |
| **#runs** | `/api/runs` | P2 表格 | 保留 kill switch（`#rn-kill-all`）位置与文案契约 |
| **#keys** | `/api/keys` | P2 表格 + P4 表单 | 保留密钥环的只读/生效摘要语义 |
| **#chat**（对话） | SSE | P4 工作台 | **ricow 独有**，fmz 无对应页 → 保留现有三栏，但视觉按 P4 改造 |
| **#settings** | `/api/settings` | P4 表单 | ricow 独有 |

### 3.3 顶栏

| fmz 顶栏 | Ricow 版 |
| -- | -- |
| Logo + 搜索框 | Logo（文字标）+ 全局搜索框（**占位**：搜索策略/会话/术语） |
| 主菜单（策略/围观/文库/社区/API文档/APP） | **不做**（社区类内容 ricow 无）→ 换成本地入口：Logo 旁的返回链接 + 面包屑在内容区 |
| 右侧图标区（主题/静音/通知/头像） | 保留 Ricow 现有的**主题切换 + 语言切换 + 帮助 + 命令面板**，位置与形态对齐 fmz 右侧图标区 |

## 4. 需求：视觉规范（设计令牌）

### 4.1 主题策略的**关键决策**

Ricow 现有三主题（`dark` / `light` / `red`），`config_file::SUPPORTED_THEMES` 是白名单单一来源。

**fmz 的实际形态是浅色**（白顶栏 + 白侧栏 + 浅灰内容区 + 绿色主色）。因此：

- **方案 A（推荐）**：把 fmz 浅色作为**默认主题**，即把 `light` 主题重做成 fmz 规范并设为默认；`dark` / `red` 保留但按同一套令牌结构重做（保持深色可用）。
- **方案 B**：新增第四主题 `fmz`，原三主题不动 —— 但会多一套变量，且主题白名单要扩。

> **倾向 A**：Ricow 的 `dark` 是当前默认，fmz 是浅色产品。保留"三主题"这一既有能力、把 fmz 规范落到 `light` 上，改动面最小、用户拿到手就是 fmz 的样子。但**这会让默认主题从 dark 变成 light**，属于可见行为变更 → **需用户拍板**（见第 8 节）。

### 4.2 令牌清单（`style.css` 变量层）

现有 `style.css` 有 `:root` / `[data-theme="light"]` / `[data-theme="red"]` 三套变量，共 22 个。新令牌按 fmz 实测值落地：

```css
/*  fmz 规范 —— 内容区 */
--bg:            #F4F6F7;   /* 页面底 */
--panel:         #FFFFFF;   /* 卡片/顶栏/侧栏 */
--raised:        #FAFAFA;   /* 表头 */
--border:        #F0F0F0;   /* 卡片描边 */
--border-strong: #D9D9D9;   /* 输入框描边 */
--fg:            #333333;   /* 正文 */
--fg-2:          rgba(51,51,51,0.65);  /* 次级 */
--fg-3:          rgba(51,51,51,0.45);  /* 三级/面包屑 */
--fg-4:          rgba(51,51,51,0.25);  /* placeholder */
--accent:        #1CAF83;   /* 主色 */
--accent-soft:   #E8FCF7;   /* 选中底 */
--accent-dim:    #17976F;
--on-accent:     #FFFFFF;
--radius:        4px;
--radius-lg:     6px;
--control-h:     32px;
--topbar-h:      64px;
--sider-w:       160px;
--menu-item-h:   48px;
```

**注意两条既有红线**：
1. **涨 = `--error` / 跌 = `--accent`**（Ricow 约定，A 股习惯）。但 fmz 主色是绿色 #1CAF83，若沿用同一 `--accent`，**跌的绿色会与主色撞色**，视觉上分不出主色与跌势。→ 令牌层需把「主色」与「涨跌色」拆开：`--accent` 专主色，涨跌另用 `--up`/`--down`（涨红跌绿，遵守 A 股约定），并在计划里显式记录这个**刻意偏离 fmz** 的决定及理由。
2. **新主题变量必须在 `:root` / `light` / `red` 三处都加**（034 定的纪律），漏一处就是某个主题下颜色失联。

### 4.3 组件映射（fmz → Ricow 现有类名）

| fmz 组件 | Ricow 目标类名 | 改动要点 |
| -- | -- | -- |
| `.ant-layout` 三段式 | `#app` → 顶栏 + 侧栏 + 内容区 | 现有 `#sidebar` 是"导航+会话列表"合一，拆开：导航进侧栏，会话列表并入对话视图 |
| `.ant-layout-sider` 160px | `#sidebar` | 会话列表移出后侧栏只剩导航 |
| `.ant-menu-item` 48px | `#nav a` | 加左侧图标位 + 选中态 `#E8FCF7`/`#1CAF83` |
| `.ant-card` 4px 圆角 | `.sg-card` / `.mk-card` / `.keys-card` / `.settings-card` | 统一到 `--radius`，去阴影，加 `--border` |
| `.ant-table` | `.rn-table` / `.sg-apiref-table` | 表头 `#FBFBFB` + 字重 600 + `12px 16px` |
| `.ant-tabs` 50px | 新增 `.ui-tabs` | fmz 标签页是 ricow 现在没有的形态（策略视图的"默认/全部"分组） |
| `.ant-btn` 32px | `.sg-btn` / `.keys-btn` / `.settings-btn` / `.mini-btn` | 统一高度 32 + 圆角 4 + 三档（主/描边/文字） |
| `.ant-empty` | `.empty-cta` | fmz 空态是"插画 + 文案"，ricow 现有是纯文字 |
| `.ant-breadcrumb` | `.mk-crumbs` / `.sg-crumbs`（已有！） | 统一为「控制中心 / XX」两级格式 |

## 5. 需求：资产与实现约束

### 5.1 硬约束（既有纪律，本变更不得违反）

1. **加载顺序即契约**：`common.js` → `router.js` → 视图脚本 → `app.js`。新增文件必须插对位置，并同步改 `assets.rs::ASSETS`（**单一清单**，加资产只改它 + `index.html`）。
2. **042 拆分纪律**：`R.sg` 命名空间 + 状态袋 `S.st`；跨文件引用一律**延迟别名**，禁值别名。
3. **禁 `localStorage` / `sessionStorage`**；禁 `input.type=`；禁用户数据拼 HTML；禁交易所写动作（`all_js()` 扫内嵌副本，**注释含字面量也算命中**）。
4. **弹窗统一挂 `#modal-root`**（`R.modal`），各视图私有 root 已移除。`open()` 必须先清场，`closeAll()` 逐个走 `close()`。
5. **离开守卫由 router 回调**（`R.setLeaveGuard`），视图自己监听 `hashchange` 是死代码。
6. **主题白名单单一来源** `config_file::SUPPORTED_THEMES`。
7. **门禁六件**：fmt / clippy(`-D warnings`) / `cargo test --workspace --no-fail-fast` / `ci_grep_gates.sh` / `web_frontend_check.mjs` / 真机两路（e2e_web.py + CDP probe）。
8. `web_frontend_check.mjs` **零依赖、故意不进 cargo test**；**改前端两边都要跑**。

### 5.2 新增资产（规划）

| 文件 | 作用 | 加载位置 |
| -- | -- | -- |
| `ui.js` | 通用 UI 原语：`R.ui.tabs()` / `R.ui.card()` / `R.ui.empty()` / `R.ui.breadcrumb()` / `R.ui.button()` | `common.js` 之后、`router.js` 之前 |
| `dashboard.js` | 新增 `#dashboard` 视图（P1 仪表盘） | 视图脚本区 |
| `icons.js` | 内联 SVG 图标集（fmz 侧栏那套线性图标） | `common.js` 之后 |

> 图标：fmz 用 `@ant-design/icons`。ricow 不能引 CDN（CSP `script-src 'self'`），故**自绘内联 SVG**，风格对齐 fmz 的 24×24 线性图标（1.5px 线宽）。

### 5.3 结构性改动（影响既有功能，须逐一守回归）

| 改动 | 风险 | 守回归办法 |
| -- | -- | -- |
| 会话列表从 `#sidebar` 移入 `#view-chat` | `chat.js` 的 `els.list` 指向会变 | `chat.js` 里 `#session-list` 的查询改为在 `#view-chat` 内查找；CDP 断言会话列表在对话视图可见、切到别的视图不可见 |
| 侧栏只剩导航 | `#nav` 高亮逻辑（`router.js::renderNav`）不变 | 前端门 + CDP |
| 顶栏重排 | `#view-title` / `#session-title` / `#theme-select` / `#lang-toggle` / `#help-btn` / `#palette-btn` 的 id **一律不改**（`help.js`/`palette.js` 按 id 找） | `web_frontend_check.mjs` 静态引用段会扫 |
| 新增 `#dashboard` 路由 | `router.js::VIEW_NAMES` 白名单要加 | 单测 `R.parseHash` 覆盖新路由 + 非法路由回退 |
| 默认主题若改为 light | 影响所有用户首屏观感 | 需用户拍板；改后 `R.applyTheme` 的 auto 解析链要重测 |

## 6. 分阶段实施顺序

每阶段独立可验收、可回退，不做"一次大改完再看"。

| 阶段 | 内容 | 产出 | 验收判据 |
| -- | -- | -- | -- |
| **P0** | 令牌层：按 fmz 实测值重写 `:root`/`light`/`red` 三套变量 + 新增 `--up`/`--down` | `style.css` 变量区 | 三主题下所有组件**无写死颜色**（grep 扫 `#` 硬编码）；`web_frontend_check.mjs` 全绿 |
| **P1** | 骨架重排：顶栏/侧栏/内容区三段式 + 面包屑 + 会话列表搬家 | `index.html` + `style.css` + `chat.js` | CDP 截图：三段式与 fmz 量到的坐标一致（侧栏 160、顶栏 64、内容左边距 16）；六视图都能进 |
| **P2** | 通用组件层：`ui.js` 的 tabs/card/empty/button/breadcrumb | 新增 `ui.js` + `assets.rs` | `node --check` 过；组件在各视图实装无差异 |
| **P3** | 逐视图换肤：markets → strategies → runs → keys → settings | 各视图 JS 的 class 收敛 | 每视图 CDP 截图与 fmz 对应范式目视一致；功能零回归（回测/复制/参数/kill switch 全通） |
| **P4** | 新增 `#dashboard`（P1 仪表盘范式） | `dashboard.js` + `router.js` | 六→七视图；仪表盘数字**来自真实 API**，不许占位 |
| **P5** | 图标集与细节打磨 | `icons.js` + 微调 | 侧栏图标风格统一；对比度过 WCAG AA |
| **P6** | 全量回归 | — | **门禁六件全绿 + 真机两路全绿**；`cargo test --workspace` 读实际数字，**不按分项加算** |

## 7. 明确不做的事（避免范围蔓延）

1. **不做账号体系**：登录/注册/账号设置/二次验证/额度预警/授权管理/推广返佣/子账户组/设备管理/API 接口密钥页 —— fmz 有，ricow 无，不留占位。
2. **不做社区内容**：围观/文库/社区/策略广场 —— 需要内容运营，与 ricow 定位无关。
3. **不做移动端 APP**：fmz 的 `/m` 前缀是"移动端 Web"，与 ricow 的"管理台"无关。
4. **不做托管者 / 数据探索 / 账单 / 工单 / 私信 / 消息中心**。
5. **不改后端 API 契约**：纯前端换皮 + 一个新聚合接口（若仪表盘需要）。
6. **不引入 React / antd / npm 依赖**：CSP 是 `script-src 'self'`，且项目当前是零依赖原生 JS + `include_str!` 嵌入。**fmz 的样子要手写实现**，不是引入 antd。
7. **不做"像素级 1:1 抄"**：抄的是**信息架构 + 视觉规范 + 交互形态**；ricow 有 ricow 的功能，页面内容按 ricow 的真实数据填。

## 8. 需要用户拍板的三个点

| # | 问题 | 我的倾向 | 不定会怎样 |
| -- | -- | -- | -- |
| **Q1** | **默认主题**是否从 `dark` 改成 fmz 浅色？ | 改成浅色（fmz 就是浅色产品，一打开就该是那个样子） | 只能做"新增 fmz 主题"，三主题旧样保留，用户需手动切主题才看到 fmz 样子 |
| **Q2** | **顶栏那排 fmz 主菜单**（策略/围观/文库/社区/API文档/APP）要不要留占位？ | 不留（其中全是社区/内容类，ricow 无） | 顶栏右侧留一排死链接 → 用户点了是 404 |
| **Q3** | **fmz 的绿色主色 #1CAF83** 与 ricow「跌=绿」撞色怎么办？ | 拆两套：主色用 fmz 绿，涨跌另立 `--up`/`--down`（涨红跌绿不变） | 沿用同一变量 → 跌势绿色与主色同色，界面分不清主色与跌 |

## 9. 附：证据文件

| 文件 | 内容 |
| -- | -- |
| `tmp/fmz_browser/shots/m-*.png` | 18 张 fmz 实页截图 |
| `tmp/fmz_browser/fmz_pages.json` | 各页 DOM 结构 / 表格 / 卡片 / 表单控件清单 |
| `tmp/fmz_browser/fmz_pages2.json` | 消息中心 / 账号设置 / 私信 / 授权 / 返佣页结构 |
| `tmp/fmz_browser/cdp.mjs` | 通用 CDP 驱动（导航/求值/截图/点击/输入），零依赖 |
| `tmp/fmz_browser/step*.mjs` | 勘察脚本（12 个，可复跑验证） |
