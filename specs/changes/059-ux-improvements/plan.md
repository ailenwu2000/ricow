# 059 实施计划：Web UI 优化

> 需求池见 `spec.md`。本文件是**实施计划**：定范围、分批、每批的改动点与验收。
> 编号沿用 spec 里的 `UX-xx`，便于对照。

## 1. 范围与分批

需求池共 16 项。本轮实施 **13 项**（P1 全部 + P2 中选可安全实施的部分），**刻意不做 3 项**（见 §4）。

| 批次 | 需求 | 改动文件 | 说明 |
|---|---|---|---|
| **A 基础设施** | UX-09 | `common.js`、`strategies.js`、`runs.js`、`index.html` | 弹窗公共组件 —— 先做，后续 UX-16 的确认框要复用它 |
| **B 导航与标题** | UX-01 / UX-02 / UX-03 | `router.js`、`index.html`、`style.css` | 顶栏标题、文档标题、会话列表显隐 |
| **C 状态反馈** | UX-05 / UX-06 / UX-13 | `markets.js`、`runs.js`、`strategies.js`、`common.js` | 加载失败提示、空状态统一、危险操作显隐 |
| **D 导航补全** | UX-04 | `strategies.js` | 策略详情页返回入口 |
| **E 防错** | UX-16 | `strategy_form.js` / `strategies.js` | 未保存改动离开确认（复用 A 的弹窗） |
| **F P2 收尾** | UX-07 / UX-10 / UX-11 / UX-12 | `keys.js`、`help.js`、`palette.js`、`strategies.js` | 说明文案、滚动锁、常用分组、卡片展开 |

## 2. 各项实施要点

### A. UX-09 弹窗公共组件（含可访问性）

**现状**：`strategies.js:1225` 与 `runs.js:531` 各有一份 `openModal`；`modalActions` 同理。
除挂载点（`sg-modal-root` / `rn-modal-root`）外**逐字相同**。两者都缺：
- 没有 `role="dialog"` / `aria-modal="true"` / `aria-labelledby`
- 打开后焦点不进弹窗，`Tab` 会跑到背后页面
- 关闭后焦点不回到触发元素
- **Esc 监听在按钮关闭时未移除**（`onKey` 只在 Esc 分支里 `removeEventListener`，点取消/主钮关闭则泄漏）

**做法**：
1. `common.js` 新增 `R.modal = { open(titleText), actions(card, primaryKey) }`。
2. 统一挂载到 `index.html` 新增的 `#modal-root`（`app` 内、与 `#palette`/`#help-drawer` 同级）。
3. `open` 返回 `{ card, close }`，内部：
   - `role="dialog"` + `aria-modal="true"` + `aria-labelledby`（标题元素 id）
   - 打开时把 `document.activeElement` 存起来，焦点移到弹窗内第一个可聚焦元素
   - `keydown` 监听：Esc 关闭；Tab 在弹窗内循环
   - `close()` 里**统一移除监听**并回焦
4. `strategies.js` / `runs.js` 删掉各自的 `openModal`/`modalActions`，改为经 `R.modal` 调用（保留各自 `#sg-modal-root` / `#rn-modal-root` 为空容器或直接删掉改用 `#modal-root`；**优先统一用 `#modal-root`**）。

**验收**：`web_frontend_check.mjs` 全绿；CDP 断言 Tab 不跳出弹窗、Esc 关闭、关闭后 `document.activeElement` 回到触发钮。

### B. UX-01 / UX-02 / UX-03 导航与标题

**现状**：`#session-title` 由 `chat.js` 写会话名，**六视图下都显示**；`document.title` 恒为「Ricow」；`#session-list` 六视图常驻。

**做法**：
1. `index.html` 顶栏新增 `<span id="view-title" class="view-title"></span>`，放在 `#session-title` 之前。
2. `router.js`：
   - 新增视图名表 `VIEW_LABELS`（zh/en 各一份，与 `#nav a` 的 `data-zh`/`data-en` **同文案**；静态标签复用 DOM 属性，不抄第二份 —— 优先从 `#nav a` 读取）。
   - `applyRoute` 里：写 `#view-title`；`document.title = 视图名 + " · Ricow"`。
   - `#session-title` 的显隐：给 `<body>` 或 `#app` 加 `data-view="<name>"`，CSS 控制 `#session-title` 只在 `[data-view="chat"]` 下显示。
3. `style.css`：
   - `.view-title` 样式（比 `session-title` 更醒目）。
   - `#session-title` 在非 chat 视图隐藏。
   - 会话列表 `#session-list` 在非 chat 视图收起（`display: none`）。

**验收**：六视图截图逐个核对顶栏标题；CDP 断言 `document.title` 与 `Ricow.currentView` 一致。

### C. UX-05 / UX-06 / UX-13 状态反馈

**UX-05 市场加载失败**：
- `markets.js` 列表加载：包一层超时（如 10s），超时或 `R.api` 抛错时渲染错误盒：`t("mkLoadFailed") + 原因` + 「重试」按钮。
- 保留现有「显示全部 / 过滤视野」控件可用（不给假控件）。

**UX-06 / UX-13 空状态**：
- `runs.js`：`#rn-kill-all` 在实例数为 0 时 `hidden = true`。
- 各视图空状态统一走 `R.emptyState(text, cta)`（已存在，无 cta 返回 `.empty` 灰字 div）；策略「我的」分组的空横线「—」改为统一组件 + 明确文案。

**验收**：断网或改 hosts 模拟不可达，市场视图出现错误盒与重试按钮；空状态截图核对。

### D. UX-04 策略详情返回入口

- `strategies.js` 详情骨架（`$("sg-d-...")` 区域）标题行加返回按钮 `‹ 返回策略列表`，点击 `R.navigate('#strategies')`。
- 文案走 `data-zh`/`data-en`。

**验收**：CDP 点返回按钮回到列表。

### E. UX-16 未保存离开确认

**做法**：
- 在策略视图注册一个 `hashchange` 钩子：若 `st.detailCtx` 存在且 `updateDirty()` 判定为脏（有未保存改动），且新 hash 不是当前策略详情，则弹确认框（复用 `R.modal`）。
- 用户选「留下」→ `R.navigate` 回详情；选「离开」→ 放行（并清掉脏标记？**不清** —— 只解除本次拦截）。
- 保存成功后脏标记自然解除。
- **注意**：不能拦截 `#strategies/{同 id}`；不能干扰命令面板/帮助抽屉（它们不改 hash）。

**验收**：改参数后点侧栏「市场」，出现确认框；选留下仍在详情页。

### F. P2 收尾

- **UX-07**：`keys.js` 页头加一句说明，解释「密钥环（左侧列表）」与「当前生效段（`[ai]`/`[exchange]`）」的区别。
- **UX-10**：`help.js` 打开抽屉时给 `<body>` 加 `overflow: hidden`，关闭时移除。
- **UX-11**：`palette.js` 结果顶部加「常用操作」分组（新建会话 / 打开策略 / 打开运行 / 打开密钥）。
- **UX-12**：策略卡片描述加展开/收起按钮（不跳转详情）。

**验收**：CDP 逐项走查。

## 3. 验收方式（每批完成后）

| 门 | 命令 |
|---|---|
| 格式 | `cargo fmt --all -- --check` |
| 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings` |
| 单测 | `cargo test --workspace --no-fail-fast` |
| 安全红线 | `bash scripts/ci_grep_gates.sh` |
| 前端门 | `node scripts/web_frontend_check.mjs` |
| 官方冒烟 | `e2e_web.py`（干净 root + 无 AI 密钥 + `show_all_pairs=true`） |
| CDP 真机 | `scripts/probe_web_ui.mjs` |

前端是 `include_str!` 嵌入，**改完必须 `cargo build`** 才生效。

## 4. 刻意不做（本轮）

| 需求 | 原因 |
|---|---|
| UX-08 设置页扩充（把语言/主题移入设置页） | 属于功能新增而非体验修补；顶栏已有语言/主题入口，移入会引入第二份真相源，另开变更做 |
| UX-14 引导条进度化 | 需要后端暴露"完成到第几步"的事实；当前引导条判据是服务端探针合成，改造成进度态属新功能 |
| UX-15 英文长文案截断 | 纯 CSS 微调，收益低、需逐个元素核对，留到下一轮统一做 |

## 4'. 与计划的偏差（实施后补记）

1. **UX-04、UX-10 经核对无需改动**：详情页早有 `#sg-back` 面包屑；帮助抽屉早有 `body.help-open{overflow:hidden}` 与焦点管理。spec.md 已订正。
2. **UX-16 的挂载点从 `hashchange` 改成 router 的 `R.setLeaveGuard`**：自己监听 `hashchange` 是**永远不触发的死代码** —— router.js 排在所有视图脚本之前加载，它的监听先跑，轮到本文件时视图已被 `deactivate`。四道静态门全绿，只有 CDP 真点一次才现形（详见 `converge.md` §4.2）。
3. **`R.modal.open` 增加"开新窗先清场"**：计划里没写。实践中被拦两次会在 `#modal-root` 里摞 3 层 dialog，`querySelector` 取到上一层的控件，断言全成假象。
4. **探针 `scripts/probe_web_ui.mjs` 修了 3 处既存的假绿**：`window.Ricow.router` 从未导出（三条导航恒短路）、058 段按 `#rn-modal-root` 定位（容器已按设计移除）、053 段末尾没还原导致后续段被守卫拦住。

> 完整结果与逐条真机数字见 `converge.md`。

## 5. 纪律约束（改前端必须遵守）

- 服务端文本一律 `textContent`，**不拼 `innerHTML`**。
- 静态标签走 `data-zh`/`data-en`；动态文案进 `R.TEXT.zh` 与 `R.TEXT.en` **成对**加。
- 不写 `localStorage` / `sessionStorage`（命令面板"常用操作"用内存，不持久化）。
- 新增 DOM id 不得与同文件已有 id 重复（`web_frontend_check.mjs` G 段会抓）。
- 不新增直连交易所的写按钮（只读红线）。
