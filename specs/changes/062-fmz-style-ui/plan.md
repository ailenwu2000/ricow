# 062 实施计划：fmz 风格 Web UI 重做

> 配套规格：[`spec.md`](./spec.md)。权威原则见 `specs/constitution.md`。
> **本计划待用户过目 + 拍板 Q1/Q2/Q3 后才开工。**

## 0. 开工前必须落地的三件事

| # | 待拍板项 | 落地方式 | 未定的后果 |
| -- | -- | -- | -- |
| Q1 | 默认主题是否改为 fmz 浅色 | 改 `config_file::SUPPORTED_THEMES` 顺序 + `R.applyTheme` 的默认解析；**注意 `SUPPORTED_THEMES` 是白名单单一来源**，改它要连带看 `ricow.toml` 的 `[ui].theme` 默认值与 `settings.js` 的下拉 | 返工 P0 + P1 |
| Q2 | 顶栏 fmz 主菜单是否留占位 | 不留 → 顶栏右侧改为本地入口；留 → 需给死链加"未开放"提示 | 顶栏重排返工 |
| Q3 | fmz 绿 vs「跌=绿」撞色 | 拆 `--accent`（主色）/ `--up` / `--down` 三变量 | 涨跌色与主色语义混淆，全局返工 |

**建议**：Q1 改浅色、Q2 不留、Q3 拆分。以下计划按此假设写。若用户另有决定，只影响 §1 的取值，不影响阶段划分。

---

## 1. P0 令牌层（地基，必须最先做）

### 1.1 目标

`style.css` 的三套主题变量按 fmz 实测值重写，并新增尺寸类令牌，让后续组件只引变量、不写死。

### 1.2 变量重写（`style.css` 顶部 `:root` / `[data-theme="light"]` / `[data-theme="red"]`）

**浅色（fmz 规范，`:root` 与 `[data-theme="light"]` 保持一致）**

```
--bg            #F4F6F7     页面底（fmz rgb(244,246,247)）
--panel         #FFFFFF     卡片/顶栏/侧栏
--raised        #FAFAFA     表头 / 次级面
--border        #F0F0F0     卡片描边
--border-strong #D9D9D9     输入框描边
--fg            #333333     正文（fmz rgba(51,51,51,.88) 落到实色，深底上再调）
--fg-2          rgba(51,51,51,.65)
--fg-3          rgba(51,51,51,.45)
--fg-4          rgba(51,51,51,.25)
--accent        #1CAF83     主色
--accent-soft   #E8FCF7     选中底
--accent-dim    #17976F
--on-accent     #FFFFFF
--up            #F5222D     涨（红，A 股约定；沿用既有 --error 语义，保留别名）
--down          #0D9B6C     跌（绿，与主色区分：--down 用更暗更冷的绿）
--radius        4px
--radius-lg     6px
--control-h     32px
--topbar-h      64px
--sider-w       160px
--menu-item-h   48px
```

**深色（`[data-theme="dark"]` 承接现有 `:root` 默认）**

保持现有深色系不推翻（Ricow 用户已习惯），但**结构对齐浅色**（同样补 `--radius` / `--control-h` / `--topbar-h` / `--sider-w` / `--menu-item-h` / `--fg-2` / `--fg-3` / `--fg-4` / `--border-strong` / `--up` / `--down`）。主色在深底上要提亮一档保证对比度（`#1CAF83` 在 `#0E1116` 上对比度约 7.4:1，可用；描边按钮的白字需复核）。

**红色（`[data-theme="red"]`）**

同上，补齐同名变量；主色改红调（`#FF8F7D`）但 `--accent-soft` 要配等比浅底。

> **纪律**：三处**必须同名同义**，漏一处 = 某个主题下该颜色失联。034 定的规矩，本轮新增的 13 个令牌一个都不能漏。

### 1.3 颜色语义拆分（Q3 落地）

现状：`--accent` 兼作"主色"与"跌势绿"。改法：

- `--accent` = **只做主色**（选中态、主按钮、图标强调）
- `--down` = 跌（绿）；`--up` = 涨（红）
- 既有引用跌势的 22 处（K 线、持仓盈亏、成交方向）**改引 `--down`**；引用主色的改引 `--accent`
- `--error` 保留为**语义红**（错误/危险操作），`--up` 可与它同值但**分开命名**，避免"错误=上涨"的语义混淆

> ⚠️ 这是一次**跨文件的机械替换**，必须逐处核。当前引用点集中在 `style.css` 的 `.mk-*`（行情/订单簿）、`.sg-*`（策略回测指标）、`.rn-*`（运行）、`.card-ai`（AI 气泡）。

### 1.4 尺寸令牌落地

`style.css` 里现有的写死值（`padding: 12px 16px` 之类）逐步换成 `--control-h` / `--radius` / `--menu-item-h` 等。**不追求一次全换完**，但**新增组件一律只用变量**。

### 1.5 验收

- `grep -nE "#[0-9a-fA-F]{3,8}" style.css` 结果里**只允许出现在 `:root`/`light`/`dark`/`red` 四块变量定义内**，其余为 0
- `node scripts/web_frontend_check.mjs` 全绿
- `bash scripts/ci_grep_gates.sh` 全绿
- `cargo test --workspace --no-fail-fast` 读实际数字（注意 `config_file::SUPPORTED_THEMES` 相关单测）

---

## 2. P1 骨架重排（三段式布局）

### 2.1 目标结构（对齐 fmz 实测坐标）

```
┌──────────────────────────────────────────────────────────────┐
│ HEADER 64px  [Ricow 标] [搜索框]        [主题][语言][帮助][⌘K] │ 白底
├────────────┬─────────────────────────────────────────────────┤
│ SIDER 160px│ BREADCRUMB「控制中心 / 策略库」  ← 内容区内, 左边距16 │
│  菜单(6+1) │ ┌ CARD ─────────────────────────────────────┐   │
│  48px/项   │ │ 头部: 标题 + 操作按钮组                     │   │
│  选中=绿底 │ └─────────────────────────────────────────────┘   │
│            │ ┌ CARD ────────────────┐┌ CARD ──────────────┐   │
│            │ └──────────────────────┘└────────────────────┘   │
└────────────┴─────────────────────────────────────────────────┘
```

### 2.2 改动清单

| 文件 | 改动 | 注意 |
| -- | -- | -- |
| `index.html` | ① `#app` 拆成 `#topbar` + `#sidebar` + `#main` 三段（现结构已是这三块，**主要是语义与样式对齐**）② `#sidebar` 里的 `<ul id="session-list">` **移入 `#view-chat`** ③ `#nav` 每项加图标位（`<span class="nav-ico">`）④ 顶栏加搜索框骨架 ⑤ 各视图容器前加面包屑位 `<div class="crumbs" id="crumbs">` | `#nav` 的 `href="#xxx"` **一律不改**（router 靠它高亮）；顶栏各控件 **id 一律不改**（help.js / palette.js 按 id 找） |
| `style.css` | ① `#sidebar` 宽 160px、只留导航 ② `#nav a` 改 48px 行高 + 图标位 + 选中态 `#E8FCF7`/`#1CAF83` ③ `#topbar` 高 64px 白底 ④ 新增 `.crumbs` 面包屑样式 ⑤ `#main` 内容区底 `#F4F6F7` + 左内边距 16px | 现有 `#sidebar` 的"导航+会话列表"布局规则要拆开重写 |
| `chat.js` | `els.list` 的 `getElementById("session-list")` **仍可用**（id 不变，只是父节点变了）→ 实际**可能无需改**；但 `els.newBtn`（`#new-session`）同理 | 先读代码确认 `els` 是按 id 取还是按父子关系取 |
| `router.js` | ① `VIEW_NAMES` 加 `"dashboard"` ② `renderNav` 不用改（它遍历 `#nav a`）③ `viewLabel` 不用改 | 加路由要补 `R.parseHash` 单测覆盖 |

### 2.3 会话列表搬家的**真实影响**

`#session-list` 现在是 `#sidebar` 的子节点。搬到 `#view-chat` 后：

- **优点**：符合 fmz 的"侧栏只有导航"；策略/密钥页不再被聊天列表挤占（这正是 059 UX-03 想解决的）
- **代价**：对话视图的左栏没了 → 会话列表要在对话视图内重新布局（fmz 没有对应物，Ricow 独有形态）
- **落地**：对话视图改成 fmz 的 **P4 工作台**形态 —— 左侧会话列表条（对话视图内）+ 中间对话流 + 右侧只读面板。**对话视图整体是一个独立的 P4 页**，与其他视图的"内容区"平级。

> 这一步要改 `index.html` 里 `#view-chat` 的内部结构（`.chat-col` / `#panels` 的容器关系），是本阶段**风险最高**的一处，单独走 CDP 走查。

### 2.4 验收

- CDP 截图：侧栏宽 160、顶栏高 64、内容区左边距 16（用 `getBoundingClientRect` 断言，**不靠目视**）
- 六个视图都能进入、都能退出、导航高亮正确
- 对话视图：会话列表可见、点会话能切换；切到策略视图：会话列表不可见
- 命令面板（⌘K）跳转各视图正常
- 帮助抽屉（`help.js`）的"当前视图"高亮正确

---

## 3. P2 通用组件层（`ui.js`）

### 3.1 定位

`window.Ricow.ui` 命名空间，提供跨视图复用的 UI 原语。**不引入 antd**，手写实现 fmz 的形态。

### 3.2 API 草案

```js
R.ui = {
  card(opts)      // { title, extra:[按钮], body:Node|string(纯文本), className }
  tabs(opts)      // { items:[{key,label,count}], active, onChange }
  empty(opts)     // { text, cta:{label,onClick} }   ← fmz 的插画+文案空态
  breadcrumb(opts)// { items:[{label,href}] }
  button(opts)    // { kind:'primary'|'default'|'text'|'danger', icon, onClick }
  icon(name, size)// → inline SVG 节点
};
```

### 3.3 硬约束

1. **只产 DOM 节点，不拼 HTML 字符串**（用户数据拼 HTML 是红线）。文本一律 `textContent`。
2. **不引入新的存储**（禁 `localStorage`/`sessionStorage`）。
3. `ui.js` 必须在 `common.js` 之后、`router.js` 之前加载（它不注册视图，但要能用 `R.t()`）。
4. `assets.rs::ASSETS` 追加一条，并**同步 `index.html` 的 `<script>`**（顺序即契约）。
5. `assets.rs` 里有个**顺序测试**（断言加载顺序），新增文件要一并更新。

### 3.4 验收

- `node --check ui.js` 通过
- `web_frontend_check.mjs` A 段（语法编译）份数从 16 → 17，B 段双语字典计数相应更新
- 组件在各视图实装无样式差异

---

## 4. P3 逐视图换肤

### 顺序：markets → strategies → runs → keys → settings（从简到繁）

| 视图 | 目标范式 | 具体动作 | 风险 |
| -- | -- | -- | -- |
| **markets** | P2 + 图表 | 现有 `.mk-card` → fmz 卡片；`.mk-crumbs` → 面包屑统一格式；列表页与详情页都按 P2 布局；K 线区保持 `lightweight-charts` 不动 | 低（视觉为主） |
| **strategies** | P2 表格 + P3 卡片墙 + P4 编辑器 | 列表态改成 fmz 的"卡片头(操作按钮组) + 标签页(默认/内置) + 筛选 + 表格"；编辑器（`strategy_editor.js`）按 P4 工具条 + 标签页（代码/参数/说明）；**参数页的"未保存改动"清单必须原样保留**（053 的成果不能被换皮冲掉） | **高**（126KB + 3 个子文件，功能最密） |
| **runs** | P2 表格 | `.rn-table` 表头改 `#FBFBFB` + 字重 600；**`#rn-kill-all` 按钮位置改到卡片头右侧**（fmz 的操作按钮在卡片头），文案与短语契约**一字不改**；实例数为 0 时仍隐藏 | 中（kill switch 是危险操作） |
| **keys** | P2 + P4 | `.keys-card` 改 fmz 卡片；`.keys-list` 改表格形态；`.keys-form` 按 P4 表单；**密钥只读红线与"生效摘要"语义保持** | 中 |
| **settings** | P4 表单 | `.settings-card` 改 fmz 卡片；`#set-show-all` 保留 | 低 |
| **chat** | P4 工作台 | 见 §2.3；对话气泡按 fmz 卡片化；`#composer` 按 fmz 输入区（边框 `#D9D9D9`、圆角 4、发送按钮实色主色） | **高**（SSE 流式 + 侧栏搬家） |

### 4.1 每个视图的"零回归"清单

- **markets**：K 线渲染、订单簿实时（SSE 三态 chip，**绝不静默降级成轮询**）、交易对切换
- **strategies**：新建 / 复制 / 编辑 / 保存 / 回测 / 参数改与未保存提示 / AI 评估 / 删除
- **runs**：启动 / 停止 / **kill all（短语逐字校验）** / 日志面板 / 实例数为 0 时隐藏
- **keys**：新增 / 删除 / 别名 / 生效摘要 / **只读红线（无交易所写动作）**
- **settings**：市场视野开关 / 跳密钥页
- **chat**：会话列表 / 新建 / SSE 流式 / 斜杠命令 / 术语浮层 / 起手卡 / 三个只读面板 / 离开守卫

### 4.2 验收（每个视图单独过）

- `node scripts/web_frontend_check.mjs` 全绿
- CDP 走查该视图的**全部**交互点（不是只看渲染）
- 目视对照 `tmp/fmz_browser/shots/m-*.png` 同范式页

---

## 5. P4 新增 `#dashboard` 仪表盘

### 5.1 内容（fmz 的 P1 范式，ricow 的真实数据）

| fmz 区块 | Ricow 版 | 数据源 |
| -- | -- | -- |
| 概览数字网格（策略库/交易所/推送消息/实盘/托管者/告警） | **策略数 / 运行实例数 / 密钥数 / 会话数 / 回测任务数 / 最近告警** | 各视图已有 API 聚合；**不许写死数字** |
| 快捷入口六宫格 | 新建策略 / 新建会话 / 新建回测 / 打开市场 / 打开运行 / 打开密钥 | 纯导航 |
| 开发文档卡 | 改成 Ricow 的**帮助入口**（术语表 / 用户指南 / API 说明） | `R.glossary` + `help.js` |
| 右侧账户卡 | **不做**（无账号体系） | — |
| 消息中心卡 | **不做** | — |

### 5.2 实现

- 新增 `dashboard.js`，注册 `R.views.dashboard`
- `router.js`：`VIEW_NAMES` 加 `"dashboard"`，**且放在第一位**（fmz 控制中心是默认页）
- 路由守卫：非对话视图的顶栏不显示会话名（沿用 059 UX-01 规则）
- 数字统计**必须来自真实 API**，读不到时如实显示"—"或"读不到"，**不显示 0**（这是 ricow 的诚实口径，与 fmz 的 0 展示不同 —— **刻意偏离**）

### 5.3 验收

- 首屏落 `#dashboard`
- 每个统计数字可追溯到具体 API
- API 失败时显示错误态而非 0
- 快捷入口六项都能跳

---

## 6. P5 图标与细节

### 6.1 图标集

自绘内联 SVG（24×24 viewBox，`stroke-width: 1.5`，`currentColor`），对齐 fmz 侧栏那套线性图标：

```
dashboard  实盘/运行  策略库/策略  托管者(n/a)  交易所/密钥
交易终端/市场  数据探索(n/a)  账单(n/a)  工单(n/a)  消息(n/a)
账号(n/a)  授权(n/a)  返佣(n/a)
```

只用前 6 个 + 顶栏几个（搜索/主题/语言/帮助/命令面板/通知）。

### 6.2 细节

- 面包屑：统一「控制中心 / XX」两级格式
- 空态：改成 fmz 的"插画 + 文案 + CTA"，插画用简化 SVG
- 表格：表头 `#FBFBFB` + 字重 600 + `12px 16px`；行分隔 `#F0F0F0`
- 按钮三档统一（实色主色 / 描边主色 / 文字主色）
- 对比度：正文与背景 ≥ 4.5:1（WCAG AA）

### 6.3 验收

- 图标风格统一，无混用
- 全站无写死颜色（grep 验证）
- 对比度抽测（关键文本 ≥ 4.5:1）

---

## 7. P6 全量回归（**必做，且要读实际数字**）

### 7.1 门禁六件

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings     # 迭代到 exit 0
cargo test --workspace --no-fail-fast                      # 整仓实跑, 读实际数字
bash scripts/ci_grep_gates.sh
node scripts/web_frontend_check.mjs
# 真机两路
```

### 7.2 真机两路

| 通道 | 要求 |
| -- | -- |
| **e2e** `e2e_web.py` | 仓库外 skill；`cp` 到 `tmp/`；`RICOW_E2E_ROOT` 用**环境变量**；回测段需 `show_all_pairs=true`；**干净 root**；**不能带 AI 密钥** |
| **CDP** `probe_web_ui.mjs` | 先起 headless Edge `--headless=new --remote-debugging-port=9333 --user-data-dir=<tmp>`；**同 root 不可跑第二轮** |

### 7.3 起服务的坑（踩过的，别再踩）

- 起服务用**托管后台任务**（别用 `&`，会被回收）；**别给托管任务加 `| head`**（缓冲到结束才吐 token）
- 先 `cargo build` 再起（服务锁 exe → LNK1104）
- base URL 里**不要带 `?token=`**（脚本内部会再拼一个 → 双 token → 401）
- `show_all_pairs=false` 会遮住回测段（连踩两次）

### 7.4 假绿排查（**本轮最容易被骗的地方**）

换肤动的是"外观"，而**外观对了不等于功能对了**。必须做**变异验证**：

| 弱断言形态 | 本轮风险 | 反制 |
| -- | -- | -- |
| 只验 `length>0` | 恒定占位符照样过 | 补"真的渲染了"的前置断言 |
| 对**空集合**取反 | 元素压根没渲染也绿 | 补"真的渲染了"的前置 |
| 只比**末态** | 中间那步没生效时也绿 | 带上中间态断言（`h1>h0 &&`） |
| 只数 `check(` | ≠ 运行期 PASS 数 | 文档只声明"**全绿**"，**不钉数字** |
| 按类名挑按钮 | 挑到取消钮 | 按**文案**定位 + "按钮找得到"的前置断言 |

> 059 第三轮教训：**检查器自己也会假绿**（`ci_grep_gates.sh::non_test_lines` 与 `architecture_guard.rs::brace_delta` 都用"数 `{`/`}`"跳过 `#[cfg(test)]` 块，字符串里的花括号也被数进去 → 该块之后的生产代码被整段吞掉 → 漏扫 = 假绿）。本轮若改了这两个脚本，**必须配回归测试**。

### 7.5 基线记录

- `cargo test --workspace` = **1059 passed / 0 failed / 22 ignored**（059 后基线，本轮**一律整仓实跑读出，不按分项加算**）
- `e2e_web.py` = **458 / 0**
- CDP = **全绿**（**不钉数字**）

---

## 8. 提交纪律

- **用户明确说"提交"才 commit**（≠ push）；长 message 用 `git commit -F tmp/msg.txt`
- commit message 要承载"**刻意不做**"的决策（账号体系不做、社区不做、不引 antd、涨跌色拆分等）
- 提交前 grep 旧基线数字；**带真密钥跑过后必 grep staged 密钥明文**（应为 0；密钥走 `RICOW_AI_API_KEY` + 仓库外 `D:/sunhuazhu/API-key.txt`，**不要删**）
- `git add -A` 后必看 staged（grep `tmp|target|\.log$|\.bak$|verify-`）
- 取号 = 「`ls specs/changes/` ∪ **全仓引用**」最大 + 1 → 本次取 **062**（`061` 是目录最大；`062` 未被引用占用；`architecture.md:140` 与 `:328` 的 "062" 是 `06231b`/`T062` 片段，非变更号）

---

## 9. 依赖顺序（不可并行）

```
P0 令牌层
 └→ P1 骨架三段式
      └→ P2 ui.js 通用组件
           └→ P3 逐视图换肤（markets→strategies→runs→keys→settings→chat）
                └→ P4 dashboard 新视图
                     └→ P5 图标与细节
                          └→ P6 全量回归
```

**P1 与 P2 之间有依赖**（`ui.js` 需要 fmz 的尺寸令牌才写得出对）；**P3 各视图之间可并行**（但**每改一个都要单独过前端门 + CDP**，不能攒着一起跑）。
