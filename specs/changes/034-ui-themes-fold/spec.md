# 功能规格: Web UI 主题(白色 / 红色) + 现代字体 + 历史消息两行折叠

**功能目录**: `specs/changes/034-ui-themes-fold`

**创建日期**: 2026-10-03

**状态**: 已实施(2026-10-03 当日收敛)

**输入**: 用户描述: "给当前项目的UI布局增加几个主题，白色背景的主题/红色背景的主题，同时修改当前UI的字体大小，修改成更接近现代审美的字体和大小。另外，历史对话的对话记录实现折叠功能，即每次非当前对话只显示顶多两行，其他的折叠起来，点击才可以展开。"

**需求澄清**(AskUserQuestion, 2026-10-03): ① 折叠作用于**对话流里的历史消息**(非左栏会话列表);
② 主题偏好**写进 `ricow.toml`**(与 `[ui].lang` 同源, 不用 localStorage)。

**前置依赖**: 032-web-ui-console(视图与顶栏)、033-key-manager(导航六项)。

## 一、问题与目标

1. **只有一套深色主题**: 长时间盯盘 / 不同光线环境下没有选择;
2. **字体观感偏旧**: 15px/1.65 的 system-ui 栈密度偏松、字重发虚;
3. **长消息淹没问题流**: 回测报告、长回复一落库, 往上翻全是大段等宽文字, 找不到节奏。

目标: 主题变量化并新增白色(`light`)与红色(`red`)两套; 字体栈与字号现代化;
对话流里除最新一块外的历史消息默认收成两行, 点击展开。

**范围外**: 不做"跟随系统"自动主题; 不做字号自定义; 左栏会话列表不参与折叠(用户已澄清)。

## 二、需求与验收

### FR-1 主题三选一, 配置持久化 (P1)

顶栏(语言切换钮旁)新增主题下拉: `dark`(默认) / `light`(白色) / `red`(红色)。
值存 `ricow.toml` 的 `[ui].theme`, 读写走新端点 `GET/POST /api/theme`(与 `/api/lang` 同构);
非法值 400 且不落盘; 配置文件里的非法值**硬失败**(与 lang 同纪律)。

**验收**: 切主题 → 全站(含 K 线图配色)立即变色 → 刷新后仍保持 → `ricow.toml` 出现 `theme = "..."`。

### FR-2 颜色全面变量化 (P1)

所有组件只准引用 CSS 变量, 不准写死颜色。`:root` = 深色兜底,
`[data-theme="light"]` / `[data-theme="red"]` 各覆盖一份变量(含新增的 `--raised` / `--on-accent` /
`--accent-soft(-2)` / `--danger-border` / `--overlay` / `--raise-1/2`)。

**验收**: 全文件 grep 无残留硬编码面板色; 三主题截图人工复核可读(2026-10-03 实测通过)。

### FR-3 现代字体 (P2)

正文 14px/1.6(原 15px/1.65), 字体栈 `Inter → Segoe UI Variable → 系统栈 → 苹方/微软雅黑 UI`;
等宽栈前置 Cascadia Code / JetBrains Mono; 开抗锯齿。

### FR-4 历史消息两行折叠 (P1)

对话流(`#stream`)里 `.msg` / `.line` 两类块: 除**最新一块**外, 更早的块默认收成两行
(`-webkit-line-clamp: 2`), 右下角淡出遮罩 + "··· 点击展开"提示; 点击展开、再点收起。
例外: `.menu`(宿主菜单)不折叠; 正在流式的气泡不折叠; 手动展开过的块(`data-keep-open`)
不被后续新消息自动收起; 点术语会先展开所在块; 选文字(复制)不触发手势。

**验收**: 连发两条长消息 → 第一条收成两行 → 点击展开全文 → 再点收起(2026-10-03 浏览器实测通过)。

## 三、边界与决策

- **D1 主题单一来源**: 与 `[ui].lang` 同路径(`config_file` 白名单 + `set_values` 行式编辑),
  不新增第二处状态; 主题纯外观, **不**给会话线程送控制行(与语言不同)。
- **D2 line-clamp 而非 max-height**: 用户气泡有 8px 内边距且全局 `box-sizing: border-box`,
  `max-height: 3.2em` 会把内边距算进去(两行缩成一行半); `-webkit-line-clamp` 按行盒计数, 不受干扰。
- **D3 K 线图配色**: `markets.js` 创建图表时从 CSS 变量现场取值; `ricow:theme` 事件触发
  用缓存 K 线就地重绘(不重新拉数据)。
- **D4 首屏闪变**: 主题在 boot 时经 `/api/theme` 读取后应用, 首帧永远是深色 —— 可接受
  (与语言同节拍), 不引入 localStorage 双源。

## 四、实施落点

| 层 | 文件 | 内容 |
|---|---|---|
| 配置 | `commands/config_file.rs` | `UiSection.theme` + 白名单 + 模板注释 + 3 条单测 |
| 后端 | `web/mod.rs` | `/api/theme` GET/POST + 端点测试(读写/非法 400) |
| 样式 | `web/assets/style.css` | 变量化 + 两套新主题 + 现代字体 + 折叠样式 |
| 前端 | `web/assets/index.html` | 顶栏 `#theme-select` |
| 前端 | `web/assets/common.js` | `R.theme` / `R.applyTheme` + `ricow:theme` 广播 |
| 前端 | `web/assets/chat.js` | boot 读主题 / 切换写盘 / 折叠逻辑(`foldUp`/`foldPrevious`/点击委托) |
| 前端 | `web/assets/markets.js` | 图表配色走变量 + 主题事件重绘 |

## 五、测试基线

`cargo test -p ricow --bin ricow` = **303 passed**(300 → 303, +3); 真机冒烟(临时目录 + 端口 18801):
默认 dark → POST light/red 落盘回读一致 → `blue` 400 → 三主题浏览器截图可读 → 折叠/展开交互正确。
