# 068 任务分解

- [x] T1 Rust schema 与保存链路：`catalog.rs` `ManifestParam` 加 `name_en`/`desc_en`（可选，serde default + skip_serializing_if）；`strategy_io/mod.rs` `build_manifest` 对 `Some("")` 用既有 `non_empty` 归一为 None。
- [x] T2 五个内置 toml：逐参数补 `name_en`/`desc_en`（紧跟中文字段后）；合约 3 个补顶层 `name_en`/`summary_en`/`description_en`；参数结构字段一字不动。
- [x] T3 前端：`strategy_form.js` `paramRow`/`stringRow`/`paramEntry` 双语择优；清单编辑器行加英文输入 + `collectManifest` 往返；`strategies.js` 表头列与 zh/en 词表；`style.css` 网格 6→7 列 / 3→4 列。
- [x] T4 CLI：`templates.rs` `params_text` 有 `name_en` 时显示 `键(中文名 / English)`（计划原文写 `one_param_line`，该函数实际在 AI 调参上下文且计划明确不扩 AI，故落在 `params_text`）。
- [x] T5 spec：`specs/lua-api.md` `[[params]]` 字段行补 `name_en`/`desc_en` 与回退规则。
- [x] T6 测试与验证：新增 3 项（内置参数英文非空 / 旧清单兼容 / 空串归一）+ CLI `params_text` 锚点；`cargo test -p ricow`(594) + `-p ricow_strategy`(278) 全量回归 0 失败；Web en 态浏览器验证（详情页参数表单全英文、清单编辑器英文列 + 保存往返经 API 核实）；CLI `new` 清单实测 5 内置全带英文名。
