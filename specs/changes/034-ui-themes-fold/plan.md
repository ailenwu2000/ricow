# 实施计划: 034 UI 主题 + 现代字体 + 历史消息折叠

**日期**: 2026-10-03

## 技术决策

1. **主题持久化走 `ricow.toml`**(用户选定): 新增 `[ui].theme`, 白名单 `dark|light|red`,
   `config_file` 解析硬校验(与 `[ui].lang` 同纪律), `WRITABLE` 加 `("ui","theme")`;
   Web 端新端点 `GET/POST /api/theme` 镜像 `/api/lang`, 但**不**给会话线程送控制行(纯外观)。
2. **CSS 只留变量**: 一次性的硬编码颜色收敛(`#1d232b`×16、`#06231b`×4、7 处 rgba)成
   `--raised / --on-accent / --accent-soft(-2) / --danger-border / --overlay / --raise-1/2`;
   三套主题各是一份变量表, 组件层零改动。
3. **折叠用 `-webkit-line-clamp: 2`**: 用户气泡带 8px 内边距且全局 border-box,
   `max-height` 方案会把内边距算进去(见 spec D2)。
4. **K 线图跟随主题**: `markets.js` 从 CSS 变量取色创建图表; 监听 `ricow:theme` 用缓存 K 线重绘。

## 步骤

1. `config_file.rs`: `UiSection.theme` / 白名单 / 模板 / 解析校验 / 单测(+3)
2. `web/mod.rs`: `/api/theme` 路由 + handler + 端点测试(读写/非法 400 不落盘)
3. `style.css`: 变量化替换 → `:root` 重写 + `light`/`red` 两套变量 → 现代字体 → 折叠样式
4. `index.html` / `common.js` / `chat.js`: 主题下拉 / `R.applyTheme` / boot 读主题 + 切换写盘
5. `chat.js`: `foldUp` / `foldPrevious` / `append`·`appendDelta` 钩子 / `#stream` 点击委托
6. `markets.js`: 图表配色走变量 + 主题事件重绘

## 风险与回退

- 风险: 折叠点击与术语点击 / 文字选择冲突 → 事件委托里逐类排除(术语顺带展开所在块)。
- 风险: `-webkit-line-clamp` 在富内容块(报告结构化行)上按行截断 → 展示"前两行/两行内容"可接受。
- 回退: 全部改动集中在 7 个文件, 纯前端 + 一个只读配置项, 单 commit 可整体 revert。
