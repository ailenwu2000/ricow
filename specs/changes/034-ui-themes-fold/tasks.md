# 任务清单: 034

## T1 配置层 `[ui].theme` ✅

- [x] `UiSection` 加 `theme: Option<String>`
- [x] `UI_KEYS` / `WRITABLE` 白名单加 `theme`
- [x] 模板注释(dark/light/red 说明)
- [x] 解析硬校验: 非法值报 `[ui].theme 仅接受 "dark" / "light" / "red"`
- [x] 单测: 缺失=None / `blue` 硬失败 / 三值回读 + 外科写盘保注释 + 模板合法 TOML

## T2 `/api/theme` 端点 ✅

- [x] `SUPPORTED_THEMES` / `ThemeReply` / `ThemeBody`
- [x] `get_theme`(缺省 dark) / `set_theme`(非法 400、合法 `set_values` 落盘)
- [x] 路由挂载(走统一 token 门禁)
- [x] 端点测试: 默认 dark → POST light → toml 落盘回读 → `blue` 400 且不覆盖

## T3 主题前端 ✅

- [x] `style.css`: 变量化(20 处硬编码收敛) + `[data-theme="light"]` / `[data-theme="red"]`
- [x] `style.css`: 现代字体栈 + 14px/1.6 + 抗锯齿 + mono 栈前置 Cascadia/JetBrains Mono
- [x] `index.html`: 顶栏 `#theme-select`(双语 option)
- [x] `common.js`: `R.theme` / `R.applyTheme` / `ricow:theme` 事件
- [x] `chat.js`: boot 读 `/api/theme`(失败兜底 dark) / change 即 POST / 失败回滚选项
- [x] `markets.js`: `themeColor()` + 主题事件就地重绘 K 线

## T4 历史消息折叠 ✅

- [x] `chat.js`: `foldableBlock` / `foldUp`(溢出检测, 短块不装可点) / `foldPrevious`
- [x] `append` / `appendDelta` 两个入口接钩子(最新一块始终完整可见)
- [x] `#stream` 点击委托: 展开/收起切换; 术语/按钮/菜单/选文字不抢; 术语所在块自动展开
- [x] `style.css`: `.foldable` / `.folded`(line-clamp 2) / 淡出遮罩(按 `--bg` 与 `--raised` 分面)

## T5 门禁与真机 ✅

- [x] `cargo fmt --check` 0 差异; clippy `-D warnings` 0 告警
- [x] `cargo test -p ricow --bin ricow` 303 passed(+3)
- [x] 真机冒烟: 端点 7 项 + 浏览器三主题截图 + 折叠展开交互(2026-10-03)
