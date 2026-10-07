# 043 实施计划

## 一、改动面

| 文件 | 改动 |
|---|---|
| `crates/ricow/src/web/assets/index.html` | `<head>` 里加 `<meta name="ricow-token" content="__RICOW_TOKEN__" />` |
| `crates/ricow/src/web/assets/common.js` | `R.TOKEN` 取值改为"查询串 → meta 载体" |
| `crates/ricow/src/web/assets.rs` | 加 1 条单测：载体存在 + 读取方真的读它 |
| `tmp/e2e_web.py`（并同步回 skill） | 加"模拟浏览器导航"段（303 → cookie → 渲染结果） |
| `SKILL.md`（skill） | 补记这条**只有真浏览器能发现**的坑 |
| `specs/changes/043-web-token-sse-fix/*` | 本次变更文档 |

**不动**：`auth.rs`（凭据解析语义）、`chat.js` / `markets.js` / `runs.js`（它们拼 URL 的写法
本身没错，错在 `R.TOKEN` 是空的）、`style.css`。

## 二、决策

- **D1 — 用 meta 载体而不是改 `token_of`。** 两条路都能修；选前端侧的理由：
  1. 真正错的是"没有 token 时还拼了个空的 `?token=`"，前端是缺陷发生地；
  2. `token_of` 是凭据解析，改它的"空值语义"影响到全站每一次请求（含 SSE/WS/静态资源），
     爆炸半径远大于改一处取值表达式；
  3. meta 载体不新增暴露面 —— token 本来就在同一份 HTML 的每个资源 URL 里。
- **D2 — 载体选 `<meta>` 而不是内联 `<script>`。** CSP 是 `script-src 'self'`（**无
  `unsafe-inline`**），内联脚本会被直接拦掉；`<meta>` 不受 script-src 约束，且读取方只需一次
  `querySelector`。
- **D3 — 取值顺序保持"查询串优先"。** 进程重启后 token 换新，用户点的新链接（带新 token）
  必须盖掉浏览器里残留的旧 cookie —— 这条 026 已定的语义保留；meta 只是**兜底**。
- **D4 — 回归护栏必须落在 e2e 脚本里，而不只是 Rust 单测。** Rust 单测只能证明"载体在、
  读取方读了"，证明不了"303 之后渲染出的 HTML 里真的填上了 token"。后者要发带
  `Sec-Fetch-Mode: navigate` 的请求走完整链路，故补进 `e2e_web.py`（并写回 skill，供以后复跑）。
- **D5 — 真机证据用 CDP 读 `responseStatus`。** `EventSource` 失败在页面上*没有任何可见症状*
  （`R.sse` 的 `onfail` 是空的），所以判据取浏览器自己的资源计时表，而不是"看着像连上了"。

## 三、测试策略

| 层 | 覆盖 |
|---|---|
| Rust 单测 | `index.html` 含 `meta[name="ricow-token"]`；`common.js` 含对该 meta 的读取；`render_index` 把占位符换成真 token 后 meta 内容同步变（复用既有的 `test_index_fills_token_into_asset_urls` 附近新增） |
| 冒烟脚本 | ①带 `Sec-Fetch-Mode: navigate` 请求 `/?token=T` → 303 + `Set-Cookie`；②带 cookie 请求 `/` → 200 且 HTML 里 token 载体/资源 URL 都带 token；③**反向对照**：不带 navigate 头请求 `/?token=T` → 200（不是 303），保证断言真的在测导航分支 |
| 真机（CDP） | `R.TOKEN` 非空；会话 SSE 的 `responseStatus == 200`；发一条消息后 `state.busy` 能回到 false |

## 四、风险

- **meta 里的 token 被"任何同源脚本"读到**：与现状等价（同源脚本本来就能读 `location.search`
  与资源 URL）。CSP `default-src 'self'` 挡住外源脚本。
- **`render_index` 替换是全量字符串替换**：载体与资源 URL 共用同一个占位符，故不会漏；
  单测会锁住。
- **冒烟脚本改动要同步回 skill**，否则下次复跑又用旧版（本轮已同步）。
