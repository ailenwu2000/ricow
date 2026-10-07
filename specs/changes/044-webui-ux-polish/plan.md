# 044 实施计划

## 一、改动面

| 文件 | 改动 |
|---|---|
| `crates/ricow/src/web/assets/markdown.js` | **新增**：受限 Markdown → DOM 渲染器（`R.markdown.render(text)`，返回 `DocumentFragment`） |
| `crates/ricow/src/web/assets/chat.js` | ①`renderRich` 增加"助手气泡走 markdown"的分支；②`decorateTerms` 跳过 `pre`/`code`；③日志面板搜索/过滤/跟随 |
| `crates/ricow/src/web/assets/index.html` | 加 `<script src="/markdown.js?token=...">`（排在 `chat.js` **之前**）；日志面板加工具条 |
| `crates/ricow/src/web/assets/style.css` | markdown 元素样式 + 日志工具条 + `.log-hidden` |
| `crates/ricow/src/web/assets.rs` | 加 `markdown.js` 资产；扩展加载顺序测试；调整只读面板断言（见 D4） |
| `tmp/e2e_web.py` + skill | 补 `markdown.js` 相关断言 |
| `specs/changes/044-webui-ux-polish/*` | 本变更文档 |

## 二、决策

- **D1 — 渲染器只产 DOM，不碰 `innerHTML`。** 分析文档建议"只对渲染器产物放行 innerHTML，
  同时改 `assets.rs` 白名单"。不采纳：AI 输出不可信，DOM API 路线（a）不动安全扫描的白名单，
  （b）没有"转义漏一处即 XSS"的窗口，（c）与既有纪律"用户数据只经 textContent/createTextNode"
  一致。代价是渲染器代码略长，值得。
- **D2 — markdown 只作用于助手气泡，不作用于宿主 `line` 帧。** 宿主输出是对齐纯文本
  （回测报告 `指标名: 值` / `--- 段标题 ---`），套 markdown 会把对齐打散，且会与报告结构化
  （FR-025）抢同一段文本。实现上给 `renderRich(el, md)` 加开关：**先按既有规则切出报告段**
  （报告段仍走卡片），**剩下的散文段**才按 `md` 选择 markdown 或纯文本。
  这样"助手气泡里夹着报告"这种混合内容也能正确处理。
- **D3 — 渲染时机不变**：仍在 `finishStreaming()` / `appendMessage()` 里做，流式期间只累积原文
  （避免每个 delta 重排整块 DOM 与丢失滚动位置）。
- **D4 — 只读红线的代理断言要改，但不能弱化意图。** `assets.rs` 现有断言
  `!panels.contains("<button")` 是 D17「页面无直连交易所写动作」的**代理**（当时面板里确实没有
  任何交互控件）。044 要在日志面板加搜索框/级别选择/跟随开关 —— 都是**视图控件**，不是交易动作。
  故把代理换成**精确版**：面板区里的每个 `<button>` 都必须在**显式允许清单**里
  （`log-follow` 等视图控件），且仍然禁内联事件、禁交易所写动作标识符。
  加一个交易动作按钮依然会红 —— 意图保住，例外显式化。
- **D5 — 级别过滤是启发式，文案必须自陈。** 日志行原样存储、不解析（FR-018），所以只能在行文本里
  找 `[ERROR]`/`ERROR`/`[FATAL]` 之类关键字。UI 写「级别（按关键字粗略匹配）」，不写"级别"了事。
  宿主说明行（轮转 / 读不到）不受过滤影响 —— 它们解释"日志为什么断了一截"。
- **D6 — 过滤用 `class="log-hidden"` 而不是删节点。** 改搜索词要能立刻恢复：
  删了就得重读日志（把"本地过滤"变成"重新拉取"，还会丢滚动位置）。500 个节点的样式切换成本可忽略。
- **D7 — 跟随状态机**：`follow`（用户意图）+ `atBottom()`（客观位置）两个量。
  追加后仅当 `follow` 为真才滚到底；用户上滚 → `scroll` 监听里把 `follow` 置假并计待读；
  点按钮 → 回底 + `follow = true`。原先无条件 `scrollTop = scrollHeight` 会**抢用户的滚动**。
- **D8 — 计数诚实**：`显示 M / 共 N 行`, 并注明本地最多保留 500 行（`LOG_DOM_MAX`）。

## 三、测试策略

| 层 | 覆盖 |
|---|---|
| Rust 单测 | ①`markdown.js` 在资产表且排在 `chat.js` 前（扩展现有顺序测试）；②`markdown.js` 不含 `innerHTML` / 存储 API；③面板区按钮只在允许清单内 + 无内联事件（D4） |
| `node --check` | 全部脚本（新增 markdown.js） |
| `e2e_web.py` | 首页引用 `/markdown.js`、可取到非空、`chat.js` 含渲染入口；两形态各跑一遍 |
| 真机（CDP） | 渲染正确性 + **注入负例**（`<img src=x onerror=...>`、`<script>`）必须原样成文本；日志搜索/过滤/暂停计数断言 |

## 四、风险

- **markdown 误伤**：把 `- 5%` 这类纯文本当列表。缓解：列表要求行首 `-`/`*`/`+` 后**跟空格**；
- **表格误判**：正文里的 `a | b` 会被当表格。缓解：表格要求**至少两行**且第二行是分隔行
  （`|---|`），与 CommonMark 一致。
- **术语标注范围变化**：不再进代码块 —— 这是有意的行为改进（见 FR-5），在 converge 里记录。
- **日志行多时的过滤开销**：500 节点上限内全量重扫可接受；不做增量优化（避免为假想性能负债）。
