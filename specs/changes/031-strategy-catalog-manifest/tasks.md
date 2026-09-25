# 031 任务分解: 策略目录重构 + 策略清单 + 统一注册 + UI 配置

**依据**: [spec.md](./spec.md)(FR-001..FR-020 / SC-001..SC-008) + [plan.md](./plan.md)(D1..D14) | **日期**: 2026-09-25

格式: `T编号 [P=可并行] [阶段] 描述(含精确文件路径)` —— 每条尾部标注溯源的 D / FR / SC。

**落地口径提醒(取自 plan, 非新增决策)**: 清单参数键是**表现层数据**(TOML), 不是引擎读值调用; `catalog.rs` 生产代码**不得**出现策略参数名字面量(D3/D10)。迁移内置脚本时"移动 + 改 include_str! + 改 .gitignore"是**同一原子提交**(D13)。

## 阶段 0 清单格式与解析(F0, 纯逻辑可单测)

- [x] T001 [F0] `crates/ricow/src/strategies/catalog.rs`(新)+ `strategies/mod.rs`(新): 定义 `ParamType{Float,Int,String,Bool,Enum}` / `ManifestParam{name,ty,desc,default:Option<ConfigValue>,required,options}` / `StrategyManifest{id,name,market,summary,description,suitable,unsuitable,params:Vec<(String,ManifestParam)>}` + TOML 反序列化(serde; `toml="0.8"` 已在 `crates/ricow/Cargo.toml` 就位, 零新增依赖) —— FR-001 / FR-002 / D5
- [x] T002 [P] [F0] `crates/ricow/src/strategies/catalog.rs` 单测: 全类型参数、默认值、枚举项、必填、非法 `type`、缺 `id`、`market` 与目录不一致 —— 各自按预期解析成功或报错 —— SC-002 / FR-004

## 阶段 1 迁移内置策略与目录(F1, 原子操作)

- [x] T003 [F1] 建 `strategies/spot/`、`strategies/futures/`; `git mv` 两个内置 Lua 到 `spot/`; 写 `strategies/spot/{shannon_spot_grid,paired_grid}.toml` 清单。**同一原子提交内**: ①改 4 处 `include_str!` 路径 `builtin/`→`spot/`(`commands/templates.rs:47,65` + `ricow_strategy/src/builtin_tests.rs:20,410`); ②改 `.gitignore`(忽略 `strategies/{spot,futures}/*` 但显式例外 4 个内置文件, 父目录 `!` re-include)。顺序: 先改引用与 .gitignore 再 `git mv` —— FR-006 / FR-007 / FR-008 / D13 / R1 / R2
- [x] T004 [F1] `crates/ricow/src/commands/templates.rs`: 薄封装到 `catalog.rs`(保留 `list_templates`/`read_template` 接口, 返回含中文名/类型/说明的清单文本); 单测改断言读清单中文名与参数 —— FR-012 / D6

## 阶段 2 统一加载(F2)

- [x] T005 [F2] `crates/ricow/src/commands/mod.rs`: `resolve_builtin_script` 改查 catalog(`code_of` 指向 catalog); 加内置 id 保留名校验 —— FR-009 / FR-010 / FR-011 / D9
- [x] T006 [F2] `crates/ricow/src/commands/backtest.rs` + `run.rs` + `ai/tools.rs`: `--strategy` 帮助文本与 AI 工具描述里的策略名列表改读 catalog(中文名+一句话+参数说明); **参数注入逻辑不动**(030 已删 inline_config 默认参数表) —— FR-012 / D6
- [x] T007 [F2] 真实回测验证(禁 mock): `ricow backtest --strategy paired_grid --pair SOLUSDT` 与 `--strategy shannon_spot_grid` 报告与迁移前逐位一致 —— SC-004 / FR-019 / R7

## 阶段 3 架构守卫反哺(F4)

- [x] T008 [F4] `crates/ricow_strategy/tests/architecture_guard.rs`: `STRATEGY_PARAMS` 派生自清单(读 workspace 根 `strategies/spot/*.toml`); 加"清单键 == Lua `config_*`/`num`/`cfg_str` 读取键"一致性单测 —— FR-017 / FR-018 / D10 / SC-007
- [x] T009 [F4] 三门禁: `cargo fmt --all -- --check` 零差异 + `cargo clippy --workspace --all-targets -- -D warnings` 零警告 + `cargo test --workspace` 全绿(基线 529 不倒退) —— SC-001

## 阶段 4 Web 展示与配置(F3)

- [x] T010 [F3] `crates/ricow/src/web/mod.rs`: 新增只读端点 `GET /api/strategies`(列表: id/name/market/summary/source/param_count)与 `GET /api/strategies/{id}`(详情: 中文名/说明/适用/不适用 + 参数数组 name/type/desc/default/required/options), 挂同一 `require_token` 层 —— FR-013 / FR-014 / D11
- [x] T011 [F3] `crates/ricow/src/web/assets/{index.html,app.js,style.css}`: 新增策略面板(按 market 分现货/合约两组; 点选显示详情 + 参数表单按类型渲染 f64/i64=数字、string=文本、bool=勾选、enum=下拉; 中文名+说明+默认值预填+必填标记; 双语 `data-zh`/`data-en`); **前端无直连交易所/落盘写按钮**, 表单提交走既有确认流 —— FR-015 / FR-016 / D11 / D12
- [x] T012 [F3] 前端冒烟: `ricow web` 起服务, 面板分组显示两个内置策略、参数表单预填; 编辑参数经确认后 `strategies/<name>.toml` 落盘正确 —— SC-006

## 阶段 5 文档与收敛(F5)

- [x] T013 [F5] 文档同步: `specs/{constitution,architecture,lua-api,product}.md` + `README.md`/`README_zh.md` + `website/{index,zh}.html` + `crates/ricow/src/ai/prompt.rs` + `crates/ricow_strategy/src/lib.rs` —— `strategies/builtin/` → `strategies/{spot,futures}/` 口径全部更新; `specs/changes/` 与 `specs/research/` 旧名作史实保留不回改 —— FR-020 / D14 / SC-008
- [x] T014 [F5] 收口: 记录测试基线到 `specs/roadmap.md`; grep `strategies/builtin` 在活跃文档(除 changes/research)零残留 —— SC-001 / SC-008
