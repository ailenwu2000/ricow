# 067 现货策略清单文档化 + 中英双语元数据

## 背景与需求

用户要求（2026-10-30，/plan 定稿）：

1. 策略说明全部放到 toml 清单，不放 lua 代码顶部，代码保持清洁；
2. 参数按最新参数规范核验分组（结论：两个现货清单已合规，本轮不改参数结构）；
3. 策略说明含中文名称、英文名称、中文说明、英文说明；
4. 文案纪律（驳回意见定稿）：说明必须简洁、面向用户，不出现参数键名与数学公式；键名/默认值只活在 `[[params]]`，实现细节只活在 lua 行内注释。

## 范围

- 两个现货策略：`linear_position_grid`、`smart_position`（toml 说明重写 + 英文元数据；lua 头部瘦身）。
- 共享机制：`StrategyManifest` 新增可选 `name_en`/`summary_en`/`description_en`；Web 列表/详情/清单编辑器按 `R.lang` 双语择优（缺英文回退中文）；CLI 列表/详情展示英文名与英文说明；`specs/lua-api.md` 同步。
- 不在范围：合约清单英文填充（机制已支持，后续可补）；`earn_enabled` 改 bool；`interval` 默认对齐。

## 验收

- `cargo build` + `cargo test -p ricow` + `cargo test -p ricow_strategy --test architecture_guard` 全绿。
- 新增测试：现货内置清单英文三字段非空；manifest 保存往返保留英文字段；旧 TOML 无英文字段解析不受影响；文案纪律（说明不得含任何参数键名，短键整词匹配）。
- CLI `strategies` 列表含英文名、`strategies show` 中英说明齐。
- Web EN 语言下策略列表/详情显示英文名与英文说明；清单编辑器英文字段可编辑并往返一致；ZH 不受影响。
- linear_position_grid 冒烟回测正常出报告、账本核对误差 < 0.01。
