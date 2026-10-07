# 044 收敛记录（converge）

**AI 对话 Markdown 渲染 + 日志面板搜索/过滤/跟随开关**
实施日期 2026-10-07（Windows）。来源：`tmp/webui_analysis.md` §四 P1 第 6、7 项。

## 一、需求落地对照

| 需求 | 落地 | 证据 |
|---|---|---|
| FR-1 受限 Markdown 子集 | 新增 `web/assets/markdown.js`：围栏代码块（带语言）、行内代码、加粗、斜体、删除线、链接、标题（≤3 视觉级）、无序/有序列表（一级缩进）、引用块、水平线、表格（≥2 行且第 2 行是分隔行）、段落 | E2E「`GET /markdown.js -> 200`」+ CDP 断言 `<h1>/<pre>/<code>/<table>/<strong>/<li>` 全部产出 |
| FR-2 **只作用于助手气泡** | `renderRich(el, md)` 先按既有规则切报告段（仍走卡片），**剩下的散文段**才按 `md` 选 markdown/纯文本 | CDP：宿主 `line` 帧「`- 这行不该变成列表项` / `**这行不该变粗**` / 围栏」**原样成文本**（`hostLine.mdChildren == 0`） |
| FR-3 **零 `innerHTML`** | 渲染器只用 `createElement`/`textContent`/`createTextNode`，返回 `DocumentFragment` | Rust 单测 `test_markdown_renderer_is_dom_only` + E2E「`markdown.js` 只走 DOM, 不用 innerHTML」 |
| FR-4 链接协议白名单 | 只放行 `http:`/`https:`，其余降级纯文本；外链 `target="_blank" rel="noopener noreferrer"` | E2E「`markdown.js` 有链接协议白名单」；CDP 负例 `javascript:` 未成链接 |
| FR-5 代码块内不做术语标注 | `decorateTerms` 跳过 `parent.closest("pre, code")` | CDP：`<pre>`/`<code>` 内无 `.term` 浮层 |
| FR-6 日志搜索 | 不分大小写子串匹配，改词即时生效（`applyLogFilter` 重扫全部行） | CDP：搜索「close」命中 2 行 |
| FR-7 级别过滤（**诚实口径**） | 行内关键字启发式，UI 文案「级别（按关键字粗略匹配）」；宿主说明行（轮转/读不到）**始终显示** | CDP：`error` 命中 1 / `warn` 命中 1；文案自陈见 `R.TEXT.logLevelHint` |
| FR-8 跟随/暂停 | `follow`（意图）+ `atBottom()`（客观）两个量；用户上滚自动暂停、只计待读；点按钮回底 | CDP：暂停后 `scrollTop` 保持 0 且按钮显「已暂停 · 1 条新」；点跟随回底 |
| FR-9 诚实计数 | 「显示 M / 共 N 行」+ 注明本地最多保留 500 行（`LOG_DOM_MAX`） | `R.TEXT.logCount` 含 `{shown}/{total}/{cap}` 占位 |
| FR-10 双语 | 新增控件全走 `data-zh`/`data-en` + `R.TEXT`，切语言就地更新（含按钮动态文案） | `common.js` 的 `R.TEXT.zh/en` 双语案；`syncLogTools` 在语言切换后重刷 |
| NFR-1 只读红线 | 面板仍是只读；无内联事件；无交易所写动作 | Rust 单测把代理断言换成**精确版**（每个 `<button>` 必须在显式允许清单 `log-follow` 内，见 D4） |
| NFR-2 进单一清单 | `markdown.js` 进 `assets.rs::ASSETS` **且** `index.html`，排在 `chat.js` 之前 | Rust 单测 `test_markdown_loads_before_chat_and_common_loads_first`；E2E「首页 `markdown.js` 排在 `chat.js` 之前」 |
| NFR-3 纪律扫描不变 | `markdown.js` 同受「禁浏览器存储 / 禁 `input.type=` / 禁交易所写动作」扫描 | 既有扫描测试覆盖（`all_js()` 自动纳入新资产） |
| NFR-4 无新依赖 | 手写渲染器，不引 marked.js | `assets.rs::ASSETS` 只多了 1 条内嵌项 |

## 二、门禁结果（T008，2026-10-07 实跑）

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all --check` | **0 差异** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0**；`grep -E "^(warning\|error)"` 排除 incremental-lock 噪声后**为空**（零代码告警） |
| `bash scripts/ci_grep_gates.sh` | **五条安全红线全绿** |
| `node --check`（16 份资产） | **全部通过**（含新增 `markdown.js`） |
| `cargo test --workspace` | **838 passed / 0 failed / 22 ignored**（12 个 target 全绿） |
| `e2e_web.py` 内嵌形态 | **181 PASS / 0 FAIL** |
| `e2e_web.py` 磁盘形态（`--web-assets-dir`） | **181 PASS / 0 FAIL** |

基线增量：041 的 834 → 042 **835** → 043 **836** → 044 **838**（+2，全是 `web::` 单测：`test_markdown_renderer_is_dom_only` / `test_markdown_loads_before_chat_and_common_loads_first`）。`ignored` 全程 22 未变。
`web::` 单测计数：147 → 148（042）→ 149（043）→ **151**（044）。

## 三、真机取证（headless Edge + CDP，不可用替身）

探针 `tmp/probe_044.mjs`（Node 22 内建全局 `WebSocket` 直连 CDP，**零依赖**），对着 `RICOW_ROOT=tmp/verify-044` 的真服务跑；种子数据由 `tmp/seed_044.py` 灌入。

**最终结论：问题 0 条 —— 全部通过。** 15 条 markdown 断言 + 全部日志面板断言：

- **渲染正确性**：`h1`/`pre`/`code`/`table`/`strong`/`li` 均按预期产出（`{"h":1,...,"hostLine":{...}}`）。
- **注入负例（三条，全部未执行）**：`<img src=x onerror=...>`、`<script>` 之类**原样成文本**，`<img>` 以字面串出现；页面无脚本注入。
- **宿主输出不套 markdown（FR-2）**：`hostLine.mdChildren == 0`，原文含 `- `/`**`/围栏也照样是纯文本。
- **日志面板**：搜索命中 2、`error` 命中 1、`warn` 命中 1；暂停后 `scrollTop` 保持 0 且显示「已暂停 · 1 条新」；点跟随钮回底并恢复跟随。

### 3.1 本轮「只有真机才抓得到」的两个既有缺陷

真机走查**在动 044 代码之前**就抓出两处静态/接口测试**结构上抓不到**的回归（分属 042 / 043，处置见各自 converge）：

1. **`ReferenceError: onUndo is not defined`** —— 042 拆分后 `strategy_editor.js` 漏了 `onUndo` 的跨文件导出，而外壳 `mount()` 直接把它挂到 `#sg-undo`。打开策略页即整片报错；`node --check` / `grep` / `cargo test` / Python 冒烟**全都不会红**。
2. **token 303 换发后所有 SSE 401（静默）** —— `R.TOKEN` 为空串时，`?token=` 会把 cookie 盖成空 → 事件流全部 401 且界面上看不出异常。详见 043。

→ 结论已固化为纪律：**跨文件符号缺口与「浏览器独有路径」只靠真机走查兜底**（`tmp/audit_042.py` 静态审计 + CDP 探针 + `e2e_web.py` B2 段）。

## 四、收敛期踩到的坑（供下次复用）

- **`e2e_web.py` 有两份，跑的不是「刚改的那份」。** 断言补在 skill 目录的副本里，而实际执行的是仓库 `tmp/e2e_web.py` —— 于是**两次 167 PASS 完全是假绿**（044 的 14 条断言一条都没跑）。发现方式：跑完 `grep -in markdown` 日志**一处命中都没有**，才去 `diff` 两份脚本，差异恰好就是 044 那 14 条。
  → **纪律**：每次跑 E2E 前**先 `cp <skill>/e2e_web.py tmp/`**，跑完复核日志里**确实出现**本轮新增断言的字样。已写进 `SKILL.md`。
  → 同步后真实计数：167（043 口径）→ **181**（+14 = `markdown.js` 入资产表 2 条 + 044 专属 12 条）。
- **`markdown.js` 头注释里写了 `localStorage`/`sessionStorage` 字面量** → 被纪律扫描命中（扫描是**字面量**匹配，注释也算）。改注释措辞（不写出 API 名）而不是加白名单。
- **`--web-assets-dir` 模式下新资产仍要重建**：路由表由**编译期** `ASSETS` 派生，磁盘模式只换内容来源、不换清单。新增脚本必须 `cargo build` 一次。
- **CDP 探针的「空泛通过」**：早期一条断言数 `#stream .line` 的条数（`>=1`），而宿主消息是**在我把它加进种子脚本之前**灌的 → 计数 0 时被另一条「会话已结束」告警行凑数通过。改成**按内容定位**宿主行 + 重跑种子后才有效。

## 五、与既有约束的关系

- **D17 只读红线未松**：日志工具条是**视图控件**，不是交易动作。原代理断言 `!panels.contains("<button")` 换成**显式允许清单**（`allowed_view_controls = ["log-follow"]`）+ 仍禁内联事件、仍禁交易所写动作标识符 —— 加交易按钮照样红，**例外显式化而非意图弱化**。
- **不引构建链/不引依赖**：渲染器手写，仍零 npm、零 bundler。
- **FR-018「日志原样」未动**：级别只在**前端**按关键字启发式过滤，后端返回与字面量一字未改；UI 自陈「粗略匹配」。
- **报告结构化（FR-025）与术语标注（FR-024）未被抢**：报告段先切走，markdown 只吃剩下的散文段；术语浮层跳过 `pre`/`code`（这是**有意的行为改进**）。

## 六、遗留

- **不做**完整 CommonMark（不追求 spec 全合规）：遇不认识的语法**原样显示**，不猜。
- **不做**语法高亮、日志导出/下载；不改 `LOG_TAIL_LINES`(200)/`LOG_DOM_MAX`(500)。
- 未改动日志源的读取方式（尾读/轮转/续传全不动）。

## 七、取证文件清单（均在 `tmp/`，已 gitignore）

| 文件 | 用途 |
|---|---|
| `tmp/probe_044.mjs` | CDP 真机探针（渲染正确性 + 注入负例 + 日志三件套） |
| `tmp/seed_044.py` | 灌种子（助手消息 + 宿主消息） |
| `tmp/e2e044_embed.log` / `tmp/e2e044_disk.log` | 两种形态冒烟日志（各 181 PASS / 0 FAIL） |
| `tmp/test044.log` | 全量测试原日志（12 target 汇总） |
| `tmp/clippy044.log` | clippy 原日志（仅 incremental-lock 噪声） |
