# 任务分解: 033-key-manager

**功能目录**: `specs/changes/033-key-manager`

**上游**: [spec.md](spec.md) · [plan.md](plan.md) · [converge.md](converge.md)

> 全部任务已完成(2026-10-02)。取证与偏差见 [converge.md](converge.md)。

## 阶段 1 — 配置层(密钥环的读写内核)

- [x] **T001** `commands/config_file.rs`: 新增 `AiKeyEntry` / `ExchangeKeyEntry` 结构体与 `File` 的 `ai_keys` / `exchange_keys` 字段。
- [x] **T002** `load()`: 识别顶层数组表 `[[ai_key]]` / `[[exchange_key]]`; 元素字段校验(必填 alias/provider, `env ∈ {live,demo}`, 类型必须字符串); 未知段错误文案同步列出新段名。
- [x] **T003** 块级行式编辑内核: `find_table_block` / `block_alias` / `render_ai_key_block` / `render_exchange_key_block`, 复用既有 `SetValue::render` 转义与 `write_private` 原子写 + 0600。
- [x] **T004** 对外操作: `upsert_ai_key(root, entry, prev_alias)` / `remove_ai_key(root, alias) -> bool` / `upsert_exchange_key` / `remove_exchange_key`; 同类别名唯一校验。
- [x] **T005** `template_text()`: 追加"密钥环"段落的注释说明与注释掉的示例(不产生真实条目)。
- [x] **T006** 单测: 新增/改名/替换/删除后其余内容逐字不变; 重复别名被拒; CRLF 保持; 解析非法 `env` 硬失败; 空文件首次写入。
- [x] **T007** 修 `commands/onboard.rs` 测试里 `File { .. }` 字面量(加 `..Default::default()`)。

## 阶段 2 — HTTP 层

- [x] **T008** 新增 `web/keyring.rs`: 脱敏视图复用 `keys::hint_of`(提升为 `pub(super)`), `GET /api/keys` 总览(entries + current + from_entry/from 标记 + presets)。
- [x] **T009** 别名与环境校验(空/超长 24/控制字符/重复 → 400 带 `code`); 未知别名 → 404。
- [x] **T010** 写入端点: `ai/save` `ai/use` `ai/delete` `exchange/save` `exchange/use` `exchange/delete` `clear`;
  "使用中"判定 + 删除时的幽灵凭据防护(FR-007) + 编辑使用中条目后的自动跟随(FR-008)。
- [x] **T011** `web/mod.rs`: 挂 8 条路由(在 token 中间件之内), `mod keyring;`。
- [x] **T012** 单测: 使用中判定、hint 三态、GET 响应不含明文、删除使用中条目清空生效字段、鉴权(无 token → 401)。

## 阶段 3 — 前端

- [x] **T013** 新增 `web/assets/keys.js`: 视图挂载 + 两组(11/币安)列表与详情表单 + 选用/保存/删除 + 当前生效摘要 + 双语字典。
- [x] **T014** `web/assets/index.html`: 左侧导航加「密钥」(运行与设置之间) + `#view-keys` 容器 + `keys.js` 引用; `web/mod.rs` 加静态资源常量与 handler。
- [x] **T015** `web/assets/router.js`: `VIEW_NAMES` 加 `keys`。
- [x] **T016** `web/assets/settings.js`: 移除币安/demo/AI 三张密钥卡, 保留市场视野; 加指向「密钥」页的引导条。
- [x] **T017** `web/assets/style.css`: 追加密钥页样式(两组卡片 / 左列表右详情 / 使用中徽标 / 危险删除按钮), 纯追加不改既有规则。

## 阶段 4 — 验证与收敛

- [x] **T018** `cargo build` + `cargo test` 全绿(记录基线数字变化)。
- [x] **T019** 真机冒烟: 临时数据目录起 `ricow web`, curl 跑三个用户故事 + 拒绝路径(重复别名/非法别名/未知别名/无 token);
  核对 `ricow.toml` 落盘内容与 `[ai]`/`[exchange]` 生效值; 校验响应不含明文。
- [x] **T020** 收敛: 复核 `specs/product.md` / `architecture.md` 中"设置页密钥"的现状描述, 更新 `roadmap.md`; 写 `converge.md`。
