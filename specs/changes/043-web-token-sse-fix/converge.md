# 043 收敛记录（converge）

**Web token 303 换发后 SSE/WS 全部静默 401 的修复**
实施日期 2026-10-07（Windows）。**起因**：044 的真机走查在动代码之前发现的既有回归。

## 一、缺陷本质

`web/auth.rs` 的 token 中间件取 token 的优先级是 **`Authorization: Bearer` → 查询串 `?token=` → cookie**，且**查询串优先于 cookie、并不判空**。而顶部导航（`Sec-Fetch-Mode: navigate`）会 **303** 到去掉 token 的干净地址并顺手种 `ricow_token` cookie —— 于是浏览器二次打开时，**地址栏里的 `?token=` 是空串**，空串直接**盖掉**刚种下的 cookie：

```
?token=              ← 空串（不是"没给"）
cookie: ricow_token=<正确值>   ← 被无视
→ token_of() 判定为非法 → 401
```

后果不是「页面打不开」（页面本身 200），而是 **`EventSource` / WebSocket 全部 401 且界面上毫无异样**：对话不流式、市场不实时、日志不刷新。这属于「静默失效」，比崩溃更难发现。

**为什么既有测试全绿**：Python 冒烟每次请求都**显式带上 token**，永远走不到「空 `?token=` + cookie」这条组合；`node --check` / Rust 单测 / grep 也不会碰到浏览器导航语义。**这条路径只有真浏览器走得进去**。

## 二、修复形状

| 文件 | 改动 |
|---|---|
| `web/assets/index.html` | `<head>` 加 token 载体：`<meta name="ricow-token" content="__RICOW_TOKEN__" />`（服务端与各 `<script src>` 用同一套占位符替换） |
| `web/assets/common.js` | `R.TOKEN` 改为 **查询串 → meta 载体** 的取值（带注释说明为什么不能只靠 query） |
| `web/assets.rs` | 新增单测锁住「载体存在 + 读取方真的读它 + 替换后内容正确」 |

**为什么不改中间件优先级**（例如「空查询串视为未给」）：那会改动**所有端点**的 token 判定语义，属于安全面收缩范围之外的改动；而「换发后 JS 仍能拿到 token」本就是**客户端**的职责。故取最小面：**只让前端在任何情况下都记得住 token**，服务端一行未动。

## 三、门禁结果（T006，2026-10-07 实跑）

`cargo fmt --all --check` **0 差异** / `cargo clippy --workspace --all-targets -- -D warnings` **exit 0、零代码告警** / `bash scripts/ci_grep_gates.sh` **五条红线全绿** / `node --check` 全部脚本通过 / `cargo test --workspace` **836 passed / 0 failed / 22 ignored**（041 的 834 → 042 **835** → 043 **836**，`web::` 单测 148 → **149**）。

`e2e_web.py`：159（042 口径）→ **167 PASS / 0 FAIL**，新增的 **B2 段 8 条**：

```
PASS  顶部导航带 query token -> 303
PASS  303 的 Location 已去掉 token (地址栏/历史不留)
PASS  303 顺手种了 HttpOnly 会话 cookie
PASS  非导航(脚本/SSE/curl)同请求 -> 200 不重定向 (反向对照)
PASS  带会话 cookie 取 / -> 200
PASS  首页不留字面量占位符
PASS  渲染后各资源 URL 带上 token
PASS  渲染后 meta 载体带上 token (043: SSE 靠它拿 token)
```

**要点**：第 4 条是**反向对照** —— 同一个带 query token 的请求，只要 `Sec-Fetch-Mode` 不是 `navigate`（脚本 / SSE / curl），就必须 **200 不重定向**。少了它，把 303 写成「见谁都 303」也能骗过前三条。

## 四、真机取证（T007）

headless Edge + CDP（`tmp/diag_token.mjs` / `tmp/probe_chat.mjs`），对着 `RICOW_ROOT=tmp/verify-04x` 的真服务：

- 导航进入后 `R.TOKEN` **非空**（走 meta 兜底）；
- 会话 `EventSource` 的 `performance.getEntriesByType('resource')` 里 **`responseStatus == 200`**；
- 对照：修复前同一探针量到 **401**（而带真实 token 的动态流 `responseStatus` 为 **0**，即连接被挂着而不是被拒 —— 这正是"看着像产品不响应"的成因）；
- 端到端：`/help` 得到回复、`turn_end` 释放输入框，流式链路真的活了。

> 探针坑：读 SSE **别无条件带 token** —— 合法请求会成功建立长连接，`read()` 于是不返回，看起来像"产品没响应"。用 `performance` 取 `responseStatus`，或把输出落文件，比盯回显可靠。

## 五、纪律固化

- `SKILL.md` 新增 **§1.3**：记录 303 / 空 `?token=` / SSE 401 这条陷阱、`performance` 取证法、修复形状，以及复跑方式（CDP 探针在 `tmp/`）。
- `tmp/diag_token.mjs`（复现/回归探针）与 `tmp/probe_chat.mjs`（SSE 连通性探针）留在 `tmp/`，已 gitignore。
- 记忆库追加一条长期约定：**「query 优先 cookie 且不判空」+ 前端三级取值** —— 下次改 token 相关代码前必读。

## 六、与既有约束的关系

- **未改服务端 token 判定语义**：三路优先级（Bearer → query → cookie）、navigate 才 303、SSE/WS 绝不 302 —— 一字未动。
- **未新增放行口 / 未新增依赖**：meta 载体走的是既有占位符替换机制。
- **不放松 CSP**：仍是 `default-src 'self'; script-src 'self'`（无 `unsafe-inline`），meta 载体不涉及内联脚本。

## 七、遗留

- 「空查询串覆盖 cookie」这个**服务端语义本身**保留（属刻意设计：新链接必须能盖掉浏览器里的旧 cookie，否则进程重启换 token 后旧链接就永不生效）。本变更只保证**前端不会主动送空串**。
