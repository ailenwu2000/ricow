# 041 技术方案（plan）

## 一、决策清单（拍板项，供审）

| # | 决策 | 取舍与依据 |
| :-- | :-- | :-- |
| **D1** | 把 11 个独立 handler + 路由 + 测试里的 `ALL_JS` **收敛成一份模块级资产表** `ASSETS: &[Asset]`，**路由由它循环生成** | 清单散落是本仓已验证的事故源（032 新增 `keys.js` 时三处清单集体漏扫且全绿）。本轮既要"同一批名字两种来源"，又要"路由与白名单不脱钩"，收敛到一份是唯一不会走样的做法（FR-9） |
| **D2** | **路径永不来自请求**：路由是编译期常量路径，handler 拿到的是 `&'static str` 常量名，磁盘路径 = `dir.join(常量名)` | 不需要写 `..`/编码过滤器 —— 攻击串根本没有入口（FR-2）。过滤攻击串是"猜对手怎么写"，闭合集合是"结构上不可能" |
| **D3** | 磁盘模式**缺文件 → 500 明确报错**，不静默退回内嵌 | 静默退回 = "我改了没生效"变成谜。同 040「绝不静默降级」 |
| **D4** | 目录校验放在**启动时**（存在 / 是目录 / 含 `index.html`），失败即退出 | 给错目录是配置错误，应该在终端一句话说清，而不是浏览器白屏后让人猜 |
| **D5** | `Cache-Control: no-store` **只加在磁盘来源**上 | 它正是"改完刷新"要防的东西；内嵌来源保持原有响应头，**不改既有行为**（FR-1） |
| **D6** | 目录 `canonicalize` 一次后固定使用 | 规范化路径排除 `..`/符号链接歧义；也便于日志里给出真实路径 |
| **D7** | `WebState` 加 `assets` 字段（默认 `Embedded`）+ builder 式 `with_assets_dir()`，**不改 `WebState::new` 签名** | 既有装配处与约 10 处测试调用点零改动。沿用 032 `jobs` / 040 `realtime` 的既有做法 |
| **D8** | **不用** `#[cfg(debug_assertions)]` 门控 | 用户跑 release 二进制时同样可能需要调前端；显式 flag 即人工确认，代价是启动警告（FR-5） |
| **D9** | 读盘走 `tokio::fs`（不阻塞 runtime） | 已在依赖树（tokio `features = ["full"]`），零新依赖 |
| **D10** | 内嵌来源保持**零拷贝**（返回 `&'static str`），磁盘来源才 `String` | 内嵌是生产形态，不该为调试能力付出每请求一次 163KB 拷贝 |

## 二、改动面

### 1. `crates/ricow/src/web/assets.rs`（重写）

- `Asset { name, ctype, embedded }` + 模块级 `const ASSETS: &[Asset]`（含 `style.css` 与 10 份 JS）。
  原 11 个 `const XXX_JS` 与 test-only 的 `ALL_JS` 全部并进来。
- `INDEX_HTML` 仍单独一条：它是 `/` 且要按本次 token 现填资源 URL，不是纯静态文件。
- `enum AssetSource { Embedded, Disk(PathBuf) }`（`#[derive(Clone, Default)]`，`Embedded` 为默认）。
- `pub(super) fn resolve_assets_dir(&Path) -> CoreResult<PathBuf>`：规范化 + 三条校验（D4）。
- `enum Loaded { Static(&'static str), Owned(String) }` + `text_response(body, ctype, no_store)`（D10/D5）。
- `pub(super) fn routes() -> Router<WebState>`：`/` → `index`，其余由 `ASSETS` 循环
  `route(&format!("/{name}"), get(move |st: State<WebState>| serve(st, name)))` 生成（D1/D2）。
- `index` 也要支持磁盘来源 —— **token 替换必须照做**，否则页面会带着字面量
  `__RICOW_TOKEN__` 去取资源、全部 401 白屏。
- 测试模块：`ALL_JS` 改为从 `ASSETS` 派生；三条纪律扫描的覆盖面不变（FR-8）；
  新增 `resolve_assets_dir` 的拒绝路径单测。

### 2. `crates/ricow/src/web/mod.rs`

- `WebState` 增 `assets: AssetSource`，`new` 内默认 `Embedded`（D7）。
- 新增 `pub fn with_assets_dir(mut self, dir: &Path) -> CoreResult<Self>`。
- 路由：删掉 12 条静态资源 `.route(...)`，改为 `.merge(assets::routes())`。

### 3. `crates/ricow/src/commands/web.rs`

- `WebArgs` 增 `#[arg(long, value_name = "DIR")] pub web_assets_dir: Option<PathBuf>`。
- `run()` 里：给出时 `state.with_assets_dir(dir)?` 并打印**双语警告**（FR-5）。

### 4. 文档

- `specs/architecture.md`：Web 资产一节补一句开发回路（默认内嵌 + 显式开关可热读）。
- `specs/roadmap.md`：变更表加 041 行 + 测试基线数字。
- `specs/changes/041-web-dev-loop/`：spec / plan / tasks / converge。

## 三、测试策略

| 层 | 方式 |
| :-- | :-- |
| 目录校验 | 单测：不存在 / 是文件不是目录 / 缺 `index.html` → `Err`（文案含路径）；合法目录 → `Ok(规范化路径)` |
| 资产内容 | 单测：`ASSETS` 每项非空；`style.css` / `chat.js` 等按名取到内嵌副本（`test_frontend_*` 三条扫描改用派生清单） |
| 首页 ↔ 清单 | 单测（既有）：双向对齐；路由由清单生成后，首页引用了清单外的名字会 404 → 该断言仍是必要关卡 |
| 磁盘 / 内嵌两条来源 | 端到端（`mod.rs` 测试，真实监听套接字）：磁盘模式返回磁盘内容 + `no-store`；内嵌模式返回内嵌内容；缺文件 500；白名单外 404；无 token 401 |
| 前端 | `node --check` 十份脚本（本次不改 JS，属回归） |
| 真机 | 带开关改一行 JS → 刷新即见；不带开关改磁盘文件 → 刷新看不到；官方冒烟 `e2e_web.py` 两种形态各跑一次 |

## 四、风险与对策

- **磁盘模式绕过了纪律扫描？** 覆盖面上，扫描对象是**内嵌副本**；磁盘目录按约定就是
  `crates/ricow/src/web/assets/`（即下一次编译要内嵌的同一批文件），故违规会在下一次
  `cargo build` + 测试时变红。这是"调试视图 vs 发布形态"的边界，在 `AssetSource::Disk`
  的注释里写明。
- **`format!` 生成路由路径**：`Router::route` 收 `&str`，临时 `String` 在调用内被消费，无生命周期问题。
- **闭包 handler 的可 clone 性**：闭包只捕获 `&'static str`（`Copy`），满足 axum 对
  `FnOnce(A) -> Fut + Clone + Send + 'static` 的要求。
- **`no-store` 只在内嵌路径缺失**：属于有意的不对称（D5）；写进注释，避免后人"顺手对齐"。
