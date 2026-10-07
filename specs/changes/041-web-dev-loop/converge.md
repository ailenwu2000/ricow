# 041 收敛记录（converge）

> 变更：前端开发回路（`ricow web --web-assets-dir`）
> 依据：`tmp/webui_analysis.md` 第四节 P1-4 + 第六节「第三刀」（第一刀 = 039 已实施、第二刀 = 040 已实施）
> 完成日期：2026-10-07 · 提交：待用户口令（宪法：未经用户明确说「提交」不得提交）

## 一、需求落地对照

| 需求 | 落地 | 位置 |
| :-- | :-- | :-- |
| FR-1 `--web-assets-dir` 热读；不给时行为完全一致 | `AssetSource{Embedded, Disk}`，`Embedded` 为 `Default`；内嵌路径仍返回 `&'static str`（零拷贝） | `web/assets.rs`、`commands/web.rs` |
| FR-2 文件名白名单，路径从不来自请求 | 路由由 `const ASSETS` 循环生成，handler 收 `&'static str` 常量名；磁盘路径 = `dir.join(常量名)` | `web/assets.rs::routes/load` |
| FR-3 启动时校验并规范化目录 | `resolve_assets_dir`：`canonicalize` → 是目录 → 含 `index.html`；否则 `CoreError::InvalidArgument` | `web/assets.rs`、`WebState::with_assets_dir` |
| FR-4 缺文件 500 + 明确文案，不静默退回内嵌 | `read_file` → `WebError::new(500, "--web-assets-dir 里读不到 {name} ({path}): {e}")` | `web/assets.rs::read_file` |
| FR-5 双语启动警告 | `println!` 两条，走 `t(lang, ..)` | `commands/web.rs` |
| FR-6 磁盘来源 `Cache-Control: no-store` | `text_response(body, ctype, no_store)`，`no_store` 仅磁盘为真 | `web/assets.rs` |
| FR-7 不放松既有约束 | 同一批路由 + 同一道 token 中间件；不新开端口/放行口；安全响应头照旧 | `web/mod.rs::router` |
| FR-8 纪律扫描口径不放松 | 三条扫描改从 `ASSETS` 派生，覆盖面与 032 时**逐份相同**（仍是 10 份 `.js`） | `web/assets.rs::tests::all_js` |
| FR-9 清单收敛为一份，路由由它派生 | 11 个独立 handler + 12 条手写路由 + test-only `ALL_JS` → 1 张 `ASSETS` 表 | `web/assets.rs` |

## 二、门禁结果

| 门禁 | 结果 |
| :-- | :-- |
| `cargo fmt --all -- --check` | **0 差异** |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | **exit 0、零代码告警**（本轮连 Windows 增量锁文件噪音都是 **0 条**） |
| `bash scripts/ci_grep_gates.sh` | **5/5 全绿**（AI 层零落盘 / 日志零密钥 / 无调试残留 / 落盘入口唯一 / `Instant` 无裸减法） |
| `node --check`（10 份 JS） | 全过（本次未改 JS，属回归） |
| `cargo test --workspace --locked --no-fail-fast` | **834 passed / 0 failed / 22 ignored**（829 → **+5**） |

新增的 5 条单测：`web::assets::tests::test_asset_table_is_well_formed`、
`test_resolve_assets_dir_rejects_bad_dirs`、`test_asset_source_default_is_embedded`、
`web::tests::test_assets_stay_embedded_without_the_flag`、
`web::tests::test_assets_dir_serves_disk_copy_and_never_escapes_the_table`。

## 三、真机取证（不可用替身）

### 3.1 开发回路探针 `tmp/probe_041_devloop.py` → **19 PASS / 0 FAIL**（`tmp/probe_041_out.txt`）

服务：`RICOW_ROOT=tmp/verify-041/root ricow web --no-open --port 18799 --web-assets-dir tmp/verify-041/assets`
（资产目录 = 仓库 `assets/` 的副本，**故意删掉 `runs.js`** 以取证"缺文件"路径。）

| 项 | 实测 |
| :-- | :-- |
| 改一行 JS → 同一进程立刻生效 | 改前 `common.js` 8074 字符不含标记；向磁盘追加 21 字符标记后**同一进程**再取 → 8095 字符**含标记**，`body1 != body2` |
| 磁盘来源缓存头 | `Cache-Control: no-store` ✓ |
| 缺文件 | `/runs.js` → **500**，错误体点名 `runs.js` ✓ |
| 白名单外路径 | `/nope.js`、`/../Cargo.toml`、`/style.css.bak` → 404/400，且响应体**不含 `[package]`** ✓ |
| 首页 token 替换 | 200 + 含本次 token + 无 `__RICOW_TOKEN__` 残留 ✓ |
| token 门 | 无 token → 401 且响应体为空 ✓ |

### 3.2 两种形态的行为差异（同一次会话内直接对照）

| 形态 | `/common.js` 长度 | 含标记 | `Cache-Control` |
| :-- | --: | :-- | :-- |
| 磁盘模式（18799） | 8095 字符 | **是** | `no-store` |
| 内嵌模式（18800，不带开关） | 8074 字符 | 否 | **无**（保持原行为） |

→ FR-1 的"不给开关时行为完全一致"由此坐实；`no-store` 的不对称也如设计。

### 3.3 给错目录 → 启动即失败（exit 1）

```
① 目录不存在   → 错误: invalid argument: --web-assets-dir 目录不可用(tmp/verify-041/nope): 系统找不到指定的文件。 (os error 2)
② 目录无 index.html → 错误: invalid argument: --web-assets-dir 目录里没有 index.html, 不像前端资产目录: \\?\D:\...\nohtml
```

两种都在**开浏览器之前**退出，不会变成"服务起来了但页面白屏"。

### 3.4 官方冒烟 `e2e_web.py` 两种形态各一次 → **144 PASS / 0 FAIL**（各）

| 形态 | 结果 | 备注 |
| :-- | :-- | :-- |
| 内嵌（端口 18801，`RICOW_ROOT=tmp/verify-041-e2e`） | **144 passed, 0 failed** | 与 039 基线一致 |
| 磁盘（端口 18802，`--web-assets-dir` 指向**仓库真实** `crates/ricow/src/web/assets`） | **144 passed, 0 failed** | 路线与开发时完全一致 |

磁盘模式另有一条只有它能提供的判别证据：`/runs.js` 在该模式下（仓库目录里**有**此文件）→ 200；而在 3.1 的副本（**删掉**该文件）→ 500。内嵌模式无论如何都是 200。

**一处脚本口径提醒（不是缺陷）**：`e2e_web.py` 打印的 `NNN bytes` 实为**解码后的字符数**
（如 `common.js` 显示 8074，而磁盘文件是 9730 字节 —— CJK 多字节 + CRLF 的差异）。
故**不能**用它区分两种来源；判别证据是上面的标记 / `no-store` / 缺文件 500 三项。

## 四、与既有约束的关系（逐条说明为何没有放松）

- **单二进制 / 完全本地化**：不变。`include_str!` 内嵌仍是发布形态；开关是**额外**能力，不给即回到原状。
- **前端纪律扫描（SC-011 / D17）**：覆盖对象仍是内嵌副本，扫描清单由 `ASSETS` 派生 ——
  覆盖面**与 032 时逐份相同**（`all_js()` 过滤出 10 份 `.js`），未因收敛而漏扫或多扫。
- **token 中间件 / CSP / 不新开放行口**：磁盘来源走的是同一批路由，中间件层级未动。
- **诚实性**：缺文件 500 点名文件，不静默退回内嵌 —— 与 040「绝不静默降级」同口径。
- **不在生产默认路径**：`AssetSource::default() == Embedded` 有单测锁住；显式 flag 即人工确认
  （同 `[ai].allow_custom_base_url` 的口径），故不按 debug/release 门控。

## 五、遗留与后续

- **`strategies.js`（2626 行）拆分**：本次**未做**（用户明确选择"先只做开发回路"）。
  现在这条回路已就位 —— 拆分时可"改一处 → 刷新即见"，正是它设计出来的用途。建议作为 042。
- **未做 HMR / 文件监听**：手动刷新足够（spec §二 非目标）。
- **磁盘模式与纪律扫描的边界**：磁盘热读是**调试视图**，不改变"什么会被发布"；
  磁盘目录按约定就是仓库 assets 目录，违规会在下一次 `cargo build` + 测试时变红。

## 六、取证文件清单（均在 `tmp/`，已 gitignore）

| 文件 | 内容 |
| :-- | :-- |
| `tmp/probe_041_devloop.py` | 开发回路探针（可复跑：`python -u tmp/probe_041_devloop.py <url> <token> <assets_dir>`） |
| `tmp/probe_041_out.txt` | 探针输出（19 PASS / 0 FAIL） |
| `tmp/e2e_041_embed.txt` | 内嵌模式官方冒烟（144 PASS / 0 FAIL） |
| `tmp/e2e_041_disk.txt` | 磁盘模式官方冒烟（144 PASS / 0 FAIL） |
| `tmp/web_041_disk.log` | 磁盘模式启动日志（含开发模式警告与 URL/token） |
