# 042 策略视图前端拆分（strategies.js → 4 份）

## 一、动机

`crates/ricow/src/web/assets/strategies.js` 已长到 **2626 行 / 单文件单 IIFE**，一个作用域里
塞了列表、详情、参数表单、清单编辑、Lua 编辑器、AI 改写、回测、寻优、历史九块逻辑，
外加 13 个模块级可变状态与 80 余个函数。

具体代价：

1. **改一处要在 2600 行里定位**：任何一次前端迭代（含 043 的 Markdown / 日志面板）都要先
   在巨型文件里跳转，`--web-assets-dir`（041）解决了"不用重编译"，但没解决"文件太大"。
2. **函数靠闭包隐式耦合**：谁调用谁没有任何声明，只能靠通读；跨节调用的边界是隐形的。
3. **`webui_analysis.md` §六「第三刀」**明确把"开发回路 + 拆分"并列为提速前提，041 只做了前者。

## 二、需求

### 功能需求

- **FR-1 零行为变化**：拆分后 DOM 结构、id、class、事件绑定、请求端点与请求体、渲染文案、
  错误分支逐字一致。**这不是重构的"目标"，是硬约束**：任何一处行为差异都算缺陷。
- **FR-2 四份文件**：按子视图切开，各自独立 IIFE，不共用一个作用域：
  | 文件 | 职责 |
  |---|---|
  | `strategies.js` | 外壳：i18n 键、常量、工具、共享状态袋、`mount`、通用弹窗、列表态、新建 / AI 生成 / 复制、`R.views.strategies` 注册、`applyLang` 包装 |
  | `strategy_form.js` | 详情态：`enterDetail` / 徽章 / 诚实性提示 / 参数表单 / 清单编辑器 |
  | `strategy_editor.js` | Lua 编辑器：高亮、脏状态、Tab / 撤销、保存（编译门禁 / running 锁）、AI 改写、need_keys 引导 |
  | `strategy_backtest.js` | 回测：作业轮询、三态渲染、指标卡、权益曲线、平仓明细、策略日志、寻优、会话级历史 |
- **FR-3 共享命名空间**：`window.Ricow.sg`。工具（`t`/`$`/`h`/`bi`/`setMsg`）、常量、共享状态
  **只在 `strategies.js` 定义一次**，其余三份从 `S` 上取。
- **FR-4 共享状态集中**：13 个可变状态（`mounted`/`gen`/`listData`/`detailCtx`/`paramInputs`/
  `savedCode`/`aiDraft`/`saveLocked`/`jobs`/`pollTimer`/`jobHistory`/`btChart`/`manifestRows`）
  收进单一状态袋 `R.sg.st`，**禁止任何文件自行声明同名副本**（那会让"代际令牌作废在途响应"
  这类跨文件不变量静默失效）。
- **FR-5 加载顺序**：外壳必须先于其余三份执行（`index.html` 脚本顺序即契约）；其余三份之间
  **无顺序依赖**（跨文件符号走延迟查找）。
- **FR-6 清单同步**：四份文件同时进 `assets.rs::ASSETS`（纪律扫描覆盖面）与 `index.html`；
  041 建立的双向对齐测试必须继续通过。
- **FR-7 跨文件符号用延迟查找**：跨文件调用一律经 `const fn = (...a) => S.fn(...a)` 包装，
  **不在加载期取函数值** —— 否则外壳与子文件之间的循环依赖（`mount` 绑 `onSave`，`onSave`
  调 `enterDetail`，`enterDetail` 又调 `renderJob`）会在某个加载顺序下取到 `undefined`。
- **FR-8 模块头注释**：每份文件头部注明职责边界、依赖的 `S` 符号、以及"本文件不得做什么"。

### 非功能需求

- **NFR-1 纪律扫描覆盖面不缩水**：`localStorage`/`sessionStorage`、`input.type=`、用户数据拼 HTML、
  交易所写动作四条扫描的对象从"1 份 strategies.js"变成"4 份"。
- **NFR-2 语法门禁**：4 份文件都要过 `node --check`。
- **NFR-3 无新依赖、无构建链**：仍是 `include_str!` 内嵌 + 零 npm。

## 三、非目标

- 不引入 Vue/React/打包器（`webui_analysis.md` §五明确不建议）。
- **不改任何业务逻辑**：不顺手修 bug、不调整文案、不重构算法 —— 想改的另开变更。
- 不动 DOM id / class（CSS 与 E2E 冒烟都盯着它们）。
- 不动 `style.css`。
- 不做前端单测框架（`webui_analysis.md` P2-10，需先有 CI 方案）。

## 四、验收

1. `cargo test --workspace` 全绿（基线不降）；`node --check` 4/4。
2. 官方冒烟 `e2e_web.py`：内嵌与磁盘（`--web-assets-dir`）两形态 **各 144 PASS / 0 FAIL**。
3. **逐符号核对**（脚本化，见 `tasks.md` T004）：
   - 4 份文件里引用的每个 `S.<name>` 都有定义（无未定义引用）；
   - 无残留裸状态名（`gen`/`jobs`/… 不带 `st.` 前缀的出现 = 0）；
   - 每个原文件的顶层函数都恰好落在一个文件里（无遗漏、无重复定义）。
4. 真机走查策略页四态：列表 → 新建（模板向导）→ 详情（参数表单 / 清单编辑 / 编辑器高亮 /
   保存 / AI 改写入口）→ 回测（指标卡 / 权益曲线 / 平仓明细 / 策略日志 / 寻优 / 历史），
   切换语言与主题各一次。
