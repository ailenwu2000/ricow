# 052 · 方案

## 1. 方案 1: 复制/模板新建时把清单一起落盘

**接口**: `POST /api/strategies` 请求体新增可选字段

```jsonc
{ "name": "paired_grid-1", "market": "spot", "pair": "BTCUSDT",
  "code": "<源 Lua>", "params": {}, "overwrite": false,
  "copy_from": "paired_grid" }        // ← 新增: 源策略 id
```

**服务端**(`web/strategy_io/mod.rs::save_strategy`):

1. `copy_from` 与 `overwrite: true` 同时出现 → **400**(互斥, 见 spec §四.2);
2. `catalog::find(copy_from)` 取源清单; 找不到 → **400**(中文, 点名源 id);
3. 新纯函数 `derive_manifest(src, new_id, market)` 派生副本清单:
   - `id` = 新策略名(必须, catalog 扫描期用 `id` 与文件名对齐);
   - `market` = 本次保存的市场(必须, `check_dir` 会强校验);
   - 其余(`name` / `summary` / `description` / `suitable` / `unsuitable` / `params` /
     `position_mode` / `default_leverage` / `backtest`)**逐字照搬**。
     保留源的中文名: 副本与源**是同一个策略**, 参数说明由源清单承载; 用户可在清单编辑器里改名。
   - `pair` 这类保留键**照搬**: 内置清单本就声明了它(给"交易对"字段配中文名), 表单侧自会跳过。
4. **落盘顺序**(关键):
   - a. 先写清单 `strategies/{market}/{name}.toml`(若已存在先备份成 `.<ts>.bak`, 复用
     `backup_manifest`);
   - b. 再走原有编译门禁 → `write_strategy_files`(写 `.lua` + 实例 TOML);
   - c. b 失败 → **回滚 a**: 有备份则恢复, 否则删除刚写的清单。
     这样失败态要么"什么都没写", 要么"写全了", 不会留下"有清单没脚本"或"有脚本没清单"。

**为什么清单先写**: 清单写失败时**一个字都不用写盘**(b 还没跑); 反过来(先写脚本)失败时
要恢复 `.lua`/`.toml` 两个文件(且受控覆盖下还要从 `.bak` 还原), 回滚面大得多。

**前端**(`strategies.js::openCopyModal` / `openNewFromTpl`): 请求体带上 `copy_from: s.id`,
不再依赖"参数留空"。

## 2. 方案 2: 静态提取参数骨架

**纯函数**(新模块 `crates/ricow/src/strategies/params.rs`):

```rust
pub struct ParamHint { pub key: String, pub ty: ParamType, pub default: Option<ConfigValue> }

pub fn extract_param_hints(code: &str) -> Vec<ParamHint>
```

**实现**:

1. **遮罩**: 造一份等长副本, 把**注释**(`--…` 行注释 / `--[[…]]` 块注释)与**字符串字面量**
   内容替换成空格(换行保留)。于是"在源码里找调用"这一步不会命中注释里或字符串里的同名文本。
2. **定位调用**: 在被遮罩的文本里找六种标记 —— `num(` / `cfg_str(` / `ctx:config_f64(` /
   `ctx:config_i64(` / `ctx:config_str(` / `ctx:config_bool(`。
3. **解析实参**: 回到**原文**按配平括号取出实参表(逗号切分, 跳过嵌套与字符串)。
   - `num(ctx, "k", d)` / `cfg_str(ctx, "k", d)`: 第 0 个实参必须字面是 `ctx`,
     第 1 个必须是**字符串字面量**(否则丢弃)。
   - `ctx:config_*(…)`: 第 0 个必须是字符串字面量。
4. **类型取 API 语义**(不猜): `num` / `config_f64` → `f64`; `config_i64` → `i64`;
   `cfg_str` / `config_str` → `string`; `config_bool` → `bool`。
5. **默认值取字面量**: 数字(含小数点/指数 → f64, 否则 i64) / 字符串 / `true`·`false`;
   没有字面量兜底 → 不填。
6. 跳过引擎内部键 `script` / `script_path`; 按**首次出现顺序**去重。

**端点**: `POST /api/strategies/extract-params`, 请求 `{ "code": "…" }`, 响应
`{ "params": [ { "key", "type", "default"? } ] }`。**无状态**(只吃代码, 不读盘、不落盘) ——
这样它同样作用于编辑器里**尚未保存**的草稿。

**边界(如实)**: 静态扫描只能拿到**字面量兜底**, 拿不到"语义默认"。
例: `paired_grid` 的 `min_pair_profit` 代码里写 `num(ctx,"min_pair_profit",0)`,
而清单记录的是 `0.002`(代码随后有 `if <= 0 then 2*fee_side` 的修正);
`start_price` 代码兜底 `0`, 清单标 `required`(无默认)。这两处**已知不一致, 如实保留**,
不为了让断言好看去"对齐"任何一侧。

## 3. 方案 3: AI 修改扩成能改配置

**接口**: `POST /api/strategies/{id}/ai-edit` 请求体新增 `mode: "code" | "params"`(缺省 `code`)。

**响应**: `AiEditReply` 改为两个可选字段 —— `code`(code 模式) / `params`(params 模式),
`warnings` 共用。

**params 模式的提示词**(新纯函数 `ai/provider.rs::build_params_prompt`): 给出
清单摘要 + 实例当前值 + 用户的话, 要求**只输出一个 JSON 对象**, 键只能取清单里的键。

**回包处理**:

1. 剥 ```json 围栏 → 取第一个 `{` 到最后一个 `}` → `serde_json::from_str::<Map<String,Value>>`;
   失败 → 400 `code:"params_parse"`(中文, 提示重试或改用"改逻辑")。
2. **键过滤**(`strategy_io::keep_manifest_params`): 不在清单里的键**丢弃**, 每个丢掉的键
   产出一条 warning(点名键 + 说明"新增参数必须同时改 Lua, 请改用改逻辑模式")。
3. **值转换**: 复用既有 `config_values`(拒绝 null / 数组 / 对象)。
4. 一个键都没留下 → 400(中文)。
5. **全程不写文件** —— 与 code 模式同一条红线。

## 4. 方案 4: 前端收口

- **参数引导卡**(`strategy_form.js::renderParams`): `detail.declared === false` 时,
  参数卡片末尾插一段引导 —— 文案 + 两个按钮:
  - 「从脚本识别参数」→ 取**编辑器当前内容** POST `extract-params`, 去重后预填清单行
    (键 / 类型 / 默认值 / `name` 填键名占位), 展开并滚动到「参数清单」卡;
  - 「问 AI 怎么配」→ 滚动到 AI 卡并把模式切到「调参数」。
- **AI 卡**(`strategy_editor.js`): 两个 chip「调整参数」/「改代码逻辑」(默认调参数),
  发送按钮与占位文案随模式变。params 模式返回后把值填进参数表单
  (键为 `pair` 时填 `#sg-pair`), 并提示"点保存才写盘"。
- **「其他已保存参数」**: 补一句来源说明(来自实例 TOML, 未在清单里声明)。
- **帮助抽屉**: 策略一步补一句(复制后参数表照清单画; 没清单可用「从脚本识别参数」)。

## 5. 不变量 / 回归面

- `ASSETS` 清单不变(不新增前端文件) → `test_all_js_assets_are_listed_on_index` 不受影响。
- 新文案键 zh/en 成对且**必须被引用**(`web_frontend_check.mjs` 的 B/C 段);
  `t("…")` 的键不许有孤儿。
- 不写浏览器存储、不拼 `innerHTML`、不碰交易所写动作(`assets.rs` 三份纪律扫描)。
- 新端点全部挂在既有 token 中间件之后 → 进 `e2e_web.py` 的 401 矩阵。
