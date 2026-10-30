# 067 任务分解

- [x] T1 manifest schema 扩展：`catalog.rs` `StrategyManifest` 加 `name_en`/`summary_en`/`description_en`（可选）；`strategy_io/mod.rs` `ManifestRequest`+`build_manifest` 收发英文字段；`web/strategies.rs` `StrategyRow` 加 `name_en`/`summary_en`。
- [x] T2 两个现货 toml：重写 `summary`/`description` 为简洁用户向文案（零键名零公式），新增 `name_en`/`summary_en`/`description_en`；参数区不动。
- [x] T3 两个 lua：顶部注释块压缩为 2 行标识头；落位公式/守卫/重锚条件等实现细节移至相关函数旁行内注释。
- [x] T4 前端：`common.js` 加 `R.pickL`；`strategies.js` 卡片/下拉/生成卡/编辑器表单与词表；`strategy_form.js` 详情说明行 + 编辑器回填/保存载荷；`palette.js` 搜索键并入 name_en。
- [x] T5 CLI：`templates.rs` 列表追加英文名、详情打印英文说明。
- [x] T6 spec 演进：`specs/lua-api.md` 清单字段说明更新。
- [x] T7 测试：新增 4 项锚点测试；全量回归 + 冒烟回测 + Web 双语人工验证。
  - 验证记录: 新增 4 测试全绿; `cargo test -p ricow` 588+3 通过、`ricow_strategy` 274+4 通过、架构守卫 4 通过; CLI `new` 列表英文名生效; SOLUSDT 7d 冒烟回测 49 笔成交、账本误差 0; Web 双语浏览器验证 6 项全 PASS。
