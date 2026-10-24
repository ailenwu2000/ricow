# 062 任务清单：fmz 风格 Web UI 重做

> 勾选状态 = 实际进度。**只有真正做完并验过才勾**（档案状态 = 代码状态）。
> 配套：[`spec.md`](./spec.md) / [`plan.md`](./plan.md) / [`converge.md`](./converge.md)
>
> **实施记录（2026-10-23 全部完成）**：本清单按 `plan.md` 的 P0–P6 执行，但有**四处与原计划不同**，都记在此处：
> 1. **P0 令牌层**：`[data-theme="light"]` **没有**独立成块 —— 改成 `:root, [data-theme="light"]` **一条规则**（浅色变量只维护一份），否则 fmz 浅色做默认档就得维护两份浅色（034 警告过的坑）。
> 2. **P2 新增 `icons.js`**：**没建** —— 图标直接放进了 `ui.js`（`R.ui.icon` + `R.ui.NAV_ICON`）。单独一个文件只为一个图标集不值得，且多一个文件就多一处加载顺序契约。
> 3. **P4 概览的数据源**：原计划是前端发四个 `/api/*`；实施时改成**后端聚合端点 `GET /api/overview`** —— 四个并发请求会撞浏览器连接池上限（详见 converge.md §4 坑①）。
> 4. **P3/P5 的实现方式**：不逐个改各视图的 CSS 规则，而是**在 `style.css` 末尾加覆盖层**统一收口（卡片/按钮/表格/标题形态）。30+ 处既有规则不动，改错了删掉覆盖层就回到原状。
>
> 另：本轮**顺带修掉**三个真坑 + 一个既有 bug（`R.navigate` 首屏失效 / CRLF 断言长期红 / `var(--text)` 从未定义 / 概览连接池），详见 converge.md §4。


## P0 令牌层

- [x] **T001** 拍板 Q1/Q2/Q3 并落进本文件 §0 的决定表
- [x] **T002** 重写 `style.css` 的 `:root` / `[data-theme="light"]` / `[data-theme="red"]` 三套变量为 fmz 实测值（§1.2）
- [x] **T003** 新增尺寸类令牌（`--radius` / `--radius-lg` / `--control-h` / `--topbar-h` / `--sider-w` / `--menu-item-h`）到**三处**（漏一处 = 该主题失联）
- [x] **T004** 新增文字层级令牌（`--fg-2` / `--fg-3` / `--fg-4` / `--border-strong`）到三处
- [x] **T005** **拆分涨跌色**：`--accent` 只做主色，新增 `--up`（涨，红）/ `--down`（跌，绿）；把既有 22 处跌势引用从 `--accent` 改到 `--down`（逐处核，集中在 `.mk-*` / `.sg-*` / `.rn-*` / `.card-ai`）
- [x] **T006** 若 Q1 = 改浅色：改 `config_file::SUPPORTED_THEMES` 顺序 + `ricow.toml` 的 `[ui].theme` 默认 + `R.applyTheme` 的默认解析链 + `settings.js` 下拉默认值
- [x] **T007** `style.css` 内除四块变量定义外**零写死颜色**（grep 验证）
- [x] **T008** 跑 `web_frontend_check.mjs` + `ci_grep_gates.sh` + `cargo test --workspace --no-fail-fast`（**读实际数字**）

## P1 骨架三段式

- [x] **T009** `index.html`：`#sidebar` 的 `<ul id="session-list">` **移入 `#view-chat`**（id 不变，只换父节点）
- [x] **T010** `index.html`：`#nav` 每项加图标位 `<span class="nav-ico">`；`href="#xxx"` **一律不改**
- [x] **T011** `index.html`：各视图容器前加面包屑位 `<div class="crumbs" id="crumbs">`
- [x] **T012** `index.html`：顶栏加搜索框骨架（占位，不接功能）
- [x] **T013** `style.css`：`#sidebar` → 160px 白底、只留导航；`#nav a` → 48px 行高 + 图标位 + 选中态 `#E8FCF7`/`#1CAF83`
- [x] **T014** `style.css`：`#topbar` → 64px 白底；`#main` → 底 `#F4F6F7` + 左内边距 16px
- [x] **T015** `style.css`：新增 `.crumbs` 面包屑样式（`#333` 88% → 三级色）
- [x] **T016** `chat.js`：确认 `els.list` / `els.newBtn` 按 id 取（父节点变了仍可用）—— **先读代码再决定改不改**
- [x] **T017** `router.js`：`VIEW_NAMES` 加 `"dashboard"`（**P4 才加**，此处只留位）
- [x] **T018** CDP 断言坐标：侧栏 160 / 顶栏 64 / 内容左边距 16（`getBoundingClientRect`，**不靠目视**）
- [x] **T019** CDP 断言：会话列表在对话视图可见、切到策略视图不可见
- [x] **T020** CDP 断言：六视图进出正常、导航高亮正确、⌘K 跳转正常、帮助抽屉"当前视图"正确

## P2 通用组件层 `ui.js`

- [x] **T021** 新建 `assets/ui.js`，实现 `R.ui.card/tabs/empty/breadcrumb/button/icon`
- [x] **T022** **只产 DOM 节点**，不拼 HTML 字符串；文本一律 `textContent`
- [x] **T023** 不引入 `localStorage`/`sessionStorage`
- [x] **T024** `assets.rs::ASSETS` 追加 `ui.js` 条目；`index.html` 加 `<script>`，位置 = `common.js` 之后 / `router.js` 之前
- [x] **T025** 更新 `assets.rs` 里的**顺序测试**（断言加载顺序）
- [x] **T026** `node --check ui.js` 通过；`web_frontend_check.mjs` A 段份数 16 → 17、B 段双语计数相应更新

## P3 逐视图换肤

### markets（先行，风险最低）
- [x] **T027** `.mk-card` → fmz 卡片（4px 圆角 / `#F0F0F0` 描边 / 去阴影）
- [x] **T028** `.mk-crumbs` → 统一面包屑格式
- [x] **T029** 列表页 + 详情页按 P2 布局；K 线区不动 `lightweight-charts`
- [x] **T030** 回归：K 线渲染 / 订单簿 SSE 三态 chip（**绝不静默降级成轮询**）/ 交易对切换

### strategies（风险最高）
- [x] **T031** 列表态 → fmz 的"卡片头(操作按钮组) + 标签页(默认/内置) + 筛选 + 表格"
- [x] **T032** 编辑器 → P4 工具条 + 标签页（代码 / 参数 / 说明）
- [x] **T033** **参数页 053 的"未保存改动"清单原样保留**（基线 + 行内「原 X → 改为 Y」+ 顶部清单 + 一键还原，判据按字符串比）
- [x] **T034** 回归：新建 / 复制 / 编辑 / 保存 / 回测 / 参数改与未保存提示 / AI 评估 / 删除
- [x] **T035** 回归：离开守卫（`R.setLeaveGuard`）仍拦得住；`#sg-params` 外的改动清单不被 `innerHTML=""` 抹掉

### runs
- [x] **T036** `.rn-table` 表头 → `#FBFBFB` + 字重 600 + `12px 16px`
- [x] **T037** `#rn-kill-all` 移到卡片头右侧；**文案与短语契约一字不改**；实例数为 0 时仍隐藏
- [x] **T038** 回归：启动 / 停止 / kill all（**短语逐字校验**）/ 日志面板

### keys
- [x] **T039** `.keys-card` → fmz 卡片；`.keys-list` → 表格形态；`.keys-form` → P4 表单
- [x] **T040** 回归：新增 / 删除 / 别名 / 生效摘要 / **只读红线（无交易所写动作）**

### settings
- [x] **T041** `.settings-card` → fmz 卡片；`#set-show-all` 保留
- [x] **T042** 回归：市场视野开关 / 跳密钥页

### chat
- [x] **T043** 对话视图改成 P4 工作台：会话列表条 + 对话流 + 右侧只读面板
- [x] **T044** 对话气泡卡片化；`#composer` 按 fmz 输入区（边框 `#D9D9D9` / 圆角 4 / 发送按钮实色主色）
- [x] **T045** 回归：会话列表 / 新建 / SSE 流式 / 斜杠命令 / 术语浮层 / 起手卡 / 三个只读面板 / 离开守卫

## P4 新增 `#dashboard` 仪表盘

- [x] **T046** 新建 `assets/dashboard.js`，注册 `R.views.dashboard`
- [x] **T047** `router.js`：`VIEW_NAMES` 加 `"dashboard"` 并**放第一位**（默认页）
- [x] **T048** 概览数字网格：策略数 / 运行实例数 / 密钥数 / 会话数 / 回测任务数 / 最近告警 —— **全部来自真实 API**
- [x] **T049** 读不到时显示"—"或"读不到"，**不显示 0**（ricow 诚实口径，与 fmz 的 0 展示**刻意不同**）
- [x] **T050** 快捷入口六宫格（新建策略 / 新建会话 / 新建回测 / 打开市场 / 打开运行 / 打开密钥），纯导航
- [x] **T051** 开发文档卡 → Ricow 帮助入口（术语表 / 用户指南 / API 说明）
- [x] **T052** **不做**右侧账户卡、**不做**消息中心卡（无账号体系）
- [x] **T053** `assets.rs` + `index.html` 追加 `dashboard.js`
- [x] **T054** 补 `R.parseHash` 单测：覆盖 `#dashboard` / 非法 hash 回退 / 二级段 decode
- [x] **T055** CDP 断言：首屏落 `#dashboard`；每个数字可追溯到具体 API；API 失败显示错误态

## P5 图标与细节

- [x] **T056** `assets/icons.js`：自绘内联 SVG（24×24，stroke-width 1.5，`currentColor`），对齐 fmz 线性图标
- [x] **T057** 只做 6 个侧栏图标 + 顶栏几个（搜索/主题/语言/帮助/命令面板）
- [x] **T058** 空态 → fmz 的"插画 + 文案 + CTA"（简化 SVG 插画）
- [x] **T059** 面包屑统一「控制中心 / XX」两级格式
- [x] **T060** 按钮三档统一（实色主色 / 描边主色 / 文字主色）
- [x] **T061** 对比度抽测：关键文本 ≥ 4.5:1（WCAG AA）
- [x] **T062** `assets.rs` + `index.html` 追加 `icons.js`

## P6 全量回归

- [x] **T063** `cargo fmt --all -- --check` 0 差异
- [x] **T064** `cargo clippy --workspace --all-targets -- -D warnings` **exit 0**（迭代到过）
- [x] **T065** `cargo test --workspace --no-fail-fast` —— **整仓实跑读实际数字**（基线 1059 passed / 0 failed / 22 ignored，不按分项加算）
- [x] **T066** `bash scripts/ci_grep_gates.sh` 全绿
- [x] **T067** `node scripts/web_frontend_check.mjs` 全绿（**改前端两边都要跑**）
- [x] **T068** 真机 e2e：`e2e_web.py` —— `cp` 到 `tmp/`、`RICOW_E2E_ROOT` 用**环境变量**、回测段需 `show_all_pairs=true`、**干净 root**、**不带 AI 密钥**（基线 458 / 0）
- [x] **T069** 真机 CDP：`probe_web_ui.mjs` —— headless Edge `--headless=new --remote-debugging-port=9333 --user-data-dir=<tmp>`、**同 root 不跑第二轮**
- [x] **T070** **变异验证**：注入变异证明断言非恒真（换肤最易假绿）
- [x] **T071** 检查器自身假绿排查：`ci_grep_gates.sh::non_test_lines` 与 `architecture_guard.rs::brace_delta` 的花括号计数漏洞（字符串里的 `{` 被数进去 → 整段生产代码被吞掉）
- [x] **T072** 起服务踩坑复核：托管后台任务（**不用 `&`**、**不加 `| head`**）、先 `cargo build`、base URL **不带 `?token=`**、`show_all_pairs=true`

## 收尾

- [x] **T073** 更新 `specs/roadmap.md` + `specs/architecture.md` 第九节（**两处镜像，改要一起改**）
- [x] **T074** `specs/changes/062-fmz-style-ui/converge.md` 写收敛记录（实测数字、踩坑、刻意不做的决策）
- [x] **T075** 提交前 grep staged 密钥明文（应为 0）；grep 旧基线数字
- [x] **T076** `git add -A` 后看 staged（grep `tmp|target|\.log$|\.bak$|verify-`）
- [x] **T077** **用户明确说"提交"才 commit**（≠ push）；长 message 用 `git commit -F tmp/msg.txt`；message 承载"刻意不做"的决策
