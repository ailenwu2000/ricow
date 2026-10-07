# 043 前端 token 断链修复（303 换发后 SSE/WS 全废）

## 一、动机

2026-10-07 用 headless Edge + CDP 做 042 的真机走查时，**真实浏览器**里发现：页面上所有
`EventSource` 连接全部失败（`performance.getEntriesByType('resource')` 里 4 次
`/api/sessions/<id>/events?token=` 的 `responseStatus` 全是 `0`）。

根因是一条**只读静态证据看不出来**的链路：

```
用户在浏览器打开 http://127.0.0.1:PORT/?token=<T>
   ↓  require_token: source == Query 且 Sec-Fetch-Mode: navigate
   303 Location: /            + Set-Cookie: ricow_token=<T>     (安全-11: 地址栏/历史不留 token)
   ↓  浏览器跟随
GET /  (只带 cookie)  →  index.html 由 cookie 里的 token 渲染 → 各 <script src="?token=<T>"> 正常
   ↓
common.js: R.TOKEN = new URLSearchParams(location.search).get("token") || ""
   ↓  location.search 已经没有 token 了（303 正是为了让地址栏不留它）
R.TOKEN === ""
   ↓  chat.js / markets.js / runs.js 拼 SSE/WS 地址时是 "?token=" + encodeURIComponent(R.TOKEN)
GET /api/sessions/<id>/events?token=      ← 空的 query token
   ↓  token_of: query 分支**优先于 cookie**，且不判空
401
```

即：**查询串里"空的" token 会顶掉后面那条有效的 cookie**。`token_of` 的注释写明"查询参数
优先于 cookie"，但这个优先级是给"进程重启后新链接必须盖掉旧 cookie"用的 —— 一个空值不该参与
优先级竞争。

代价（全部静默，无任何报错文案）：

1. **AI 对话收不到流式回复**：`turn_end` 帧永远不来 → `setBusy(false)` 永不执行 → 发完第一条
   消息后输入区**永久禁用**（`state.busy` 卡在 true）。
2. **日志面板不流式**（026 FR-016 的首屏 + 增量推送全废）。
3. **市场实时化（040）整条链路连不上**：K 线/盘口的 SSE 订阅全部 401，前端三态 chip 只会停在
   "连不上"。
4. **运行页的实例流**同样失效。

为什么此前没被发现：**这条路径只在"带了 `Sec-Fetch-Mode: navigate` 的浏览器导航"里出现。**
`curl` 与官方冒烟脚本 `e2e_web.py` 都不带这个头 → 不触发 303 → `location.search` 里始终有
token → 一切正常。故 `f0e65f9`（2026-10-06 12:23，审计中危修复，含 303 换发）自带的验收
"真机冒烟 144 passed / 0 failed" 结构性**覆盖不到**它，回归静默存活至今。

## 二、需求

### 功能需求

- **FR-1 保留 303 换发**：地址栏/历史不留 token 这条安全属性**不动**（安全-11 的初衷）。
- **FR-2 前端能在无 query 时取到 token**：`index.html` 里加一处服务端可填的 token 载体
  （`<meta name="ricow-token" content="__RICOW_TOKEN__">`），`common.js` 按
  "查询串 → meta 载体"顺序取值。
  - 该载体与各 `<script src="?token=">` **同源等价**：token 本来就已经出现在本页 HTML 的每个
    资源 URL 里，故**不引入任何新的暴露面**（同样受 `default-src 'self'` 与同源策略约束）。
- **FR-3 两形态都要生效**：内嵌与 `--web-assets-dir` 都走 `render_index` 替换，故 meta 载体在
  两种形态下都被填上同一个 token。
- **FR-4 回归护栏**（这条是重点 —— 没有它下次还会静默复发）：
  - **Rust 单测**：`index.html` 必须带该载体，且 `common.js` 必须真的读它（改坏任一处即红）。
  - **冒烟脚本**：`e2e_web.py` 补一段"**模拟浏览器导航**"的断言 —— 带
    `Sec-Fetch-Mode: navigate` 请求 `/?token=T`，断言 303 + `Set-Cookie`，再带 cookie 请求 `/`，
    断言**渲染出的 HTML 里 token 载体与各资源 URL 都带上了 token**。这是 API 层能复现该链路的
    唯一方式（普通 curl 复现不了）。
  - **真机走查**：浏览器里 `EventSource` 必须真的连上（用 CDP 读
    `performance.getEntriesByType('resource')` 的 `responseStatus` 为证）。

### 非功能需求

- **NFR-1 不动安全路径**：`crates/ricow/src/web/auth.rs` 的 `token_of` / `require_token`
  **一字不改**（见"非目标"）。
- **NFR-2 无新依赖、无构建链**：仍是零 npm、零 CDN。
- **NFR-3 文案不变**：不改任何用户可见文案；新增的 meta 标签不进渲染。

## 三、非目标

- **不改 `token_of` 的"空值不判空"行为**。把"空 query token"改成"视作未提供"确实也能修好，
  但那是在**凭据解析**这条最敏感的路上改语义（例如有人依赖 `?token=` → 401 的现行为）；
  真正错的是"前端在没 token 时仍然拼了个空的 `?token=`"，故在前端侧修。此处只记录该观察，
  留给后续独立变更评估。
- 不恢复"token 留在地址栏"的旧行为。
- 不动 `R.sse` 的退避/重连策略（036 已定）。
- 不处理 `/favicon.ico` 的 404（无害噪声，浏览器默认请求）。

## 四、验收

1. Rust 单测：载体 + 读取方双向锁定；`cargo test --workspace` 全绿。
2. `e2e_web.py` 新增的"模拟浏览器导航"段通过（内嵌 + 磁盘两形态）。
3. 真机（headless Edge + CDP）：
   - `R.TOKEN` 非空；
   - `/api/sessions/<id>/events?token=<真 token>` 的 `responseStatus == 200`（不再是 0）；
   - 发一条消息后能收到流式回复，输入区在 `turn_end` 后解除禁用。
4. 全量门禁（fmt / clippy / grep 红线 / node --check / workspace 测试）全绿。
