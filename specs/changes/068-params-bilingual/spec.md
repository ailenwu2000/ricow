# 068 策略参数中英文双语（params name_en/desc_en）

## 背景与需求

067 已把策略**顶层**说明做成双语（`name_en`/`summary_en`/`description_en` + 前端 `R.pickL` 择优），
但 `[[params]]` 的 `name`/`desc` 仍只有中文——英文界面下参数表单整片中文。
用户要求（2026-10-30 /plan 定稿并批准）：给出完整的中英文参数方案；架构结论 = **不拆双 toml**，
单文件 `[[params]]` 新增可选 `name_en`/`desc_en`，中文为默认语言，英文缺省回退中文（延续 067 扁平 `*_en` 模式）。

## 范围

- 5 个内置策略全部 `[[params]]` 补齐 `name_en`/`desc_en`：
  spot `linear_position_grid` / `smart_position`，futures `shannon_grid_futures` /
  `shannon_neutral_grid_futures` / `shannon_short_grid_futures`。
- 顺带补齐合约 3 个策略的顶层 `name_en`/`summary_en`/`description_en`（067 只铺了现货顶层）。
- 全链路：serde schema → 保存链路空串归一 → 前端表单/清单编辑器双语 → CLI 参数行英文名 → `specs/lua-api.md`。
- 用户自定义策略：字段可选、零改动兼容。

## 文案纪律

- `name`/`desc` 中文原文不动；`name_en`/`desc_en` 为等义简洁翻译。
- 参数英文文案允许概念词（"ATR multiplier"），禁数学公式与实现细节；避免无必要的键名原文。
- 参数区 `type`/`default`/`required`/`options`/`group` 一字不动（params.rs 一致性测试锚定）。

## 不做

- AI「自动命名」（`parse_names`）扩英文；`runs.js` 等外围页面择优（067 尾巴）。

## 验收

- 5 个内置 toml：顶层 + 全部参数均有英文字段，中文默认语言未破坏。
- 旧清单/用户策略零改动可用（serde 兼容，序列化不凭空多出 `name_en` 键）。
- en 界面参数表单、详情页、新建实盘弹窗、清单编辑器全双语；zh 界面不变。
- 新增 3 项测试（内置参数英文非空锚点 / 旧清单兼容 / 空串归一）+ 全量回归绿。
- CLI `new` 清单参数行带英文名；`specs/lua-api.md` 与本档案落档。
