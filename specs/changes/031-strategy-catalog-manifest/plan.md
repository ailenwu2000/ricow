# 031 实施计划: 策略目录重构 + 策略清单(manifest)+ 统一注册 + UI 配置

**规格**: [spec.md](./spec.md) | **来源**: 用户三条需求 + 已复核架构计划 + 用户拍板(D1/D2) | **日期**: 2026-09-25 | **分支**: `feat/031-strategy-catalog-manifest`

## 一、决策

| # | 决策 | 理由 |
|---|---|---|
| D1 | 清单文件后缀 = `<id>.toml`(用户拍板): 与实例 TOML 同后缀, 靠目录隔离(清单只在 `spot/`/`futures/`, 实例只在根) | 少一层命名负担; 目录已隔离 |
| D2 | 建实例**不**把清单默认值全写进 params(用户拍板): 缺省走 Lua `num()` 兜底, 只加"清单默认值 == Lua 默认值"一致性断言 | 不引入运行时二次读取; 防漂移靠单测 |
| D3 | 清单是**表现层数据**, 放 CLI 层(`crates/ricow/src/strategies/`): 引擎(`ricow_engine`)与绑定层(`ricow_strategy::context/lua`)**零策略参数名铁律不变** | 架构铁律(宪法二)是最高约束; 清单只描述、不参与撮合/执行 |
| D4 | 内置脚本保留**编译期嵌入**(`include_str!`), 与用户策略在统一注册表里呈现, 仅 `source` 字段区分 | 单文件分发是核心价值; 不做"内置改运行时读盘" |
| D5 | 清单 = **sidecar TOML**(策略 = `.lua` + `.toml` 两文件); 解析用 `toml = "0.8"`(已在 `crates/ricow/Cargo.toml` 就位, 零新增依赖) | 机器可读、与项目 TOML 约定一致、可与 Lua 逻辑分离、可 `include_str!` |
| D6 | 统一注册表 `catalog.rs` 取代 `templates.rs` 的 `Template{name,summary,params,code}` 数组; 三条出口(CLI 帮助 / AI 工具 / Web 端点)共用同一份 catalog | 单一来源, 三处展示的中文名/说明/参数天然一致(FR-012) |
| D7 | catalog 合并 = 内置示例(编译期嵌入)∪ 用户自写(扫描 `strategies/{spot,futures}/`); 按 `id` 去重 | 统一管理(FR-009) |
| D8 | `market` 由清单声明、由目录印证: 不一致 = 加载报错(不静默) | FR-004; 防"清单说 futures 却放进 spot"的漂移 |
| D9 | 内置 `id` 是保留名, 用户策略占用时部署/扫描报错 | FR-010; 防用户策略覆盖内置名致 `--strategy` 歧义 |
| D10 | 架构守卫黑名单 `STRATEGY_PARAMS` 改为**从清单派生** + 加"清单键 == Lua `config_*`/`num`/`cfg_str` 读取键"一致性单测 | 消除手抄漂移(FR-017/FR-018); 守卫规则 A/B 的匹配方式(只扫读/写方法调用)不变 |
| D11 | Web 新增只读端点 `GET /api/strategies` + `GET /api/strategies/{id}`, 挂同一 `require_token` 层; 前端无直连写按钮 | FR-013/FR-016; 守 026 D17 红线 |
| D12 | 配置写路径复用既有 `update_strategy_params_in` + `create`/`deploy` + 确认流, **不新增写通道** | FR-016; 少而精, 不绕确认门禁 |
| D13 | 迁移内置脚本时, **移动 + 改 4 处 `include_str!` + 改 `.gitignore` 是同一原子提交** | 编译期引用(`include_str!`)与 git 跟踪是两份隐蔽依赖, 拆开会编译崩 / git 丢文件(审核 F1/F2) |
| D14 | 文档同步面 = `specs/{constitution,architecture,lua-api,product}` + `README*.md` + `website/*.html` + `ai/prompt.rs`(AI 提示词)+ `lib.rs` 注释 | `strategies/builtin` 口径散布 14 处, 必须全量同步(审核 F4) |

## 二、改动清单

| 文件 | 改动 |
|---|---|
| `crates/ricow/src/strategies/catalog.rs`(新) | `ParamType`/`ManifestParam`/`StrategyManifest` + TOML 反序列化; catalog 合并(内置 ∪ 用户)+ `find(id)`/`list()`; **生产代码参数键全来自 TOML 数据, 不出现策略参数名字面量**(`pair`/`script` 通用键除外, D3/D10) |
| `crates/ricow/src/strategies/mod.rs`(新) | 模块声明 |
| `strategies/spot/shannon_spot_grid.toml`(新) | 香农网格清单(id/name/market/summary/params…) |
| `strategies/spot/paired_grid.toml`(新) | 配对网格清单 |
| `strategies/futures/`(新目录) | 合约策略目录(先立, 空) |
| `strategies/builtin/*.lua` → `strategies/spot/*.lua` | `git mv` 两个内置 Lua(D13) |
| `.gitignore` | `strategies/*` + `!strategies/builtin/` → 忽略 `strategies/{spot,futures}/*` 但显式例外 4 个内置文件; **父目录必须 `!` re-include**(D13) |
| `crates/ricow/src/commands/templates.rs` | 薄封装到 catalog; `include_str!` 路径 `builtin/`→`spot/`; 保留 `list_templates`/`read_template` 接口(返回含中文名/类型/说明的清单文本) |
| `crates/ricow_strategy/src/builtin_tests.rs` | `include_str!` 路径 `builtin/`→`spot/`(第 20/410 行两处, D13) |
| `crates/ricow/src/commands/mod.rs` | `resolve_builtin_script` 改查 catalog(`code_of` 指向 catalog); 内置 id 保留名校验(D9) |
| `crates/ricow/src/commands/backtest.rs` / `run.rs` | `--strategy` 帮助文本(backtest.rs:25 / run.rs:17)与 AI 工具描述里的策略名列表改读 catalog(中文名+说明); **参数注入逻辑不动**(030 已删 inline_config 默认参数表) |
| `crates/ricow/src/commands/create.rs` / `deploy.rs` | 部署/复制落盘路径指向 `strategies/{market}/` |
| `crates/ricow/src/web/mod.rs` | 新增只读端点 `GET /api/strategies` + `/api/strategies/{id}`(D11) |
| `crates/ricow/src/web/assets/{index.html,app.js,style.css}` | 新增策略面板(现货/合约分组 + 详情 + 参数表单按类型渲染 + 双语 `data-zh`/`data-en`); 无构建链(D11) |
| `crates/ricow_strategy/tests/architecture_guard.rs` | `STRATEGY_PARAMS` 派生自清单(读 workspace 根 `strategies/spot/*.toml`)+ 一致性单测(D10) |
| `specs/{constitution,architecture,lua-api,product}.md` + `README*.md` + `website/*.html` + `crates/ricow/src/ai/prompt.rs` + `crates/ricow_strategy/src/lib.rs` | `strategies/builtin/` → `strategies/{spot,futures}/` 口径同步(D14) |

## 三、风险

| # | 风险 | 处置 |
|---|---|---|
| R1 | 移动文件与 `include_str!` 编译期引用不同步 → `cargo build` 崩 | D13: 移动 + 改 4 处 `include_str!` 同一原子提交; 顺序先改引用再 `git mv`(审核 F1) |
| R2 | `.gitignore` 漏改 → 内置脚本不再被 git 跟踪 → clone 后 `include_str!` 失败 | D13: 显式例外 4 个内置文件 + 父目录 re-include; 验证 `git status` 内置脚本仍 tracked(审核 F2) |
| R3 | 清单 default vs Lua `num()` 默认值双源漂移 | D2/D10: "清单键 == Lua 读取键"一致性单测 + "默认值一致性"断言; 不改运行时 |
| R4 | 清单参数名(CLI 层)触碰架构铁律"零策略参数名" | D3: 清单是表现层数据, 引擎/绑定层不读; 守卫只扫 `get_str("...")` 等读值调用, 不扫数据声明; catalog.rs 不写参数名字面量(审核 F6) |
| R5 | 用户策略名占用内置 id 致 `--strategy` 歧义 | D9: 部署/扫描时校验保留名, 占用报错 |
| R6 | 旧部署(内置 `type` / `script_path` / 内嵌 `script`)破坏 | FR-011: 三种方式照旧加载, 不做迁移脚本; 真实回测逐位一致兜底 |
| R7 | 破坏既有 CLI / 回测行为(最高优先级) | FR-019/SC-004: `--strategy {shannon_spot_grid,paired_grid}` 真实 K 线回测报告逐位一致; 基线 529 不倒退 |

## 四、验收动作

1. `cargo fmt --all -- --check` 零差异。
2. `cargo clippy --workspace --all-targets -- -D warnings` 零警告。
3. `cargo test --workspace` 全绿, 基线 **529 passed / 0 failed / 22 ignored** 不倒退。
4. 清单解析单测(SC-002): 全类型/默认值/枚举/必填/非法类型/缺 id/市场-目录不一致 —— 按预期解析或报错。
5. 注册表单测(SC-003): 内置+用户合并、按 id 去重、内置保留名占用被拒、旧部署三种方式均可加载。
6. 真实回测(SC-004, 禁 mock): `ricow backtest --strategy paired_grid --pair SOLUSDT` 与 `--strategy shannon_spot_grid` 报告与迁移前**逐位一致**。
7. Web 鉴权(SC-005): 无 token / 错 token → `401`; 策略列表/详情的中文名与市场与清单一致。
8. 前端冒烟(SC-006): `ricow web` 面板按现货/合约分组显示两个内置策略(中文名+一句话), 点开参数表单默认值预填; 编辑参数经确认后 TOML 落盘。
9. 架构守卫(SC-007): 黑名单派生自清单; "清单键 == Lua 读取键"一致性测试通过。
10. 文档同步核对(SC-008): grep `strategies/builtin` 在 `specs/`、`README*.md`、`website/`、`ai/prompt.rs`、`lib.rs` 中零残留(仅历史档案 `specs/changes/` 与 `specs/research/` 的旧名作史实保留, 不回改)。
