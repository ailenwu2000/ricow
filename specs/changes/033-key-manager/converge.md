# 033 收敛记录: 密钥管理页(多套 AI / 币安密钥, 别名可选用可删除)

**变更**: [spec.md](./spec.md) · [plan.md](./plan.md) · [tasks.md](./tasks.md)

**日期**: 2026-10-02 · **状态**: 已实施 + 取证完成(未提交)

**交付概览**: 密钥从"唯一一份生效值"升级为**带别名的密钥环(vault)**。左侧导航新增第 6 个一级视图「密钥」
(`#keys`, 位于「运行」与「设置」之间): AI 通道与币安凭据各一组, 组内**左列条目列表 + 右列详情表单**,
每条可按别名保存 / **选用**(写入生效段) / **删除**(删除使用中条目时同步清空生效字段, 防幽灵凭据)。
密钥仍只存**唯一配置文件** `ricow.toml`(`[[ai_key]]` / `[[exchange_key]]` 两个数组表, 原子写 + 0600),
读接口只回 `***` + 末 4 位; 「设置」页不再重复承载密钥配置, 仅留市场视野 + 指向密钥页的入口。

## 一、验收标准逐条结论(SC-001~005)

| SC | 结论 | 证据 |
|:--|:--|:--|
| SC-001 页面保存 ≥2 套 AI 密钥并切换生效, 无需手改配置/重启 | ✅ | 冒烟 2–7: 新增"工作号 DeepSeek"+"备用 Kimi"两条 → 选用"备用 Kimi" → `[ai]` 段随之切换; 改名/换模型后生效值**自动跟随**; 全程只走 HTTP, 服务不重启 |
| SC-002 删除币安条目后该凭据不再出现且不再生效 | ✅ | 冒烟 9–11 + 22: 主账户(live)/测试网(demo)两条分别选用, `binance_*` 与 `demo_*` 各自生效、互不干扰; 删除使用中条目后条目消失**且生效字段被清空** |
| SC-003 `GET /api/keys` 与页面资源不含密钥全文 | ✅ | 冒烟 3/10 对响应体做全文断言, 4 组明文(`sk-work-*` / `sk-backup-*` / `LIVEKEY-*` / `LIVESECRET-*` …)一条都不出现, 只回 `***` + 末 4 位; `web/keyring.rs` 另有响应级单测覆盖 |
| SC-004 现有行为零回退 | ✅ | `[ai]` / `[exchange]` 的解析与校验规则一行未改; legacy `GET/POST /api/config/keys` 仍可用(冒烟 24); 终端向导与对话 `/keys` 未触碰; 既有测试全绿(见 §二) |
| SC-005 手工编辑 `ricow.toml` 增删 `[[ai_key]]` 后页面能正确读到 | ✅ | `load()` 新增数组表解析, 未知段报错文案同步列出新段名; 单测覆盖 CRLF 保持、逐字保留其余内容、非法 `env` 硬失败 |

## 二、门禁证据

```
cargo fmt --all -- --check                    → exit 0 (0 差异)
cargo clippy --workspace --all-targets -- -D warnings → exit 0 (0 告警)
cargo test -p ricow --bin ricow               → 300 passed / 0 failed / 1 ignored
cargo test --workspace --no-fail-fast         → 608 passed / 22 ignored; 仅 ai_live_smoke 2 例失败(环境, 见 §五.1)
```

**增量**: ricow bin 用例 **279 → 300(+21)**, 全部来自本变更 —— `commands/config_file.rs` 新增 12 例
(数组表解析/非法 env 硬失败/别名校验/增删改后其余内容逐字不变/CRLF 保持/空文件首写/空行归一化),
`web/keyring.rs` 新增 9 例(使用中判定三态 / hint 脱敏 / GET 响应全文不含明文 / 删除使用中条目清空生效字段 /
别名与环境校验码 / 改名的自我重复豁免)。ignored 数不变(22)。

## 三、真机冒烟(临时数据目录 + curl, 全 HTTP 路径)

以 `RICOW_ROOT=<临时目录>` 起 `ricow web`, 对同一份 `ricow.toml` 做 30 组对照, 现场事后已清理:

- **读**: 空配置 → 条目 0、`presets` 表齐全、`current` 为空; legacy `/api/config/keys` 仍可用。
- **写·AI**: 新增两条 → 200 且 `ricow.toml` 落 `[[ai_key]]`; 选用 → `[ai]` 生效段同步; 改"使用中"条目的模型与密钥 → 生效段跟随(不留旧值); 删除"使用中"条目 → 生效字段被清空。
- **写·币安**: 新增 live/demo 两条 → 分别选用后 `binance_key/secret` 与 `demo_key/secret` 各自生效; `clear target=live` 只清主网, demo 不动, 条目仍在。
- **拒绝路径**: 重复别名 400 `dup_alias`; 空/超长(25 字符)别名 400 `invalid_alias`; 新增 AI 缺 provider 400; 币安只给 key 不给 secret 400; `env=prod` 400; 选用不存在的别名 404; `clear target=futures` 400; 无 token / 错 token 一律 401。
- **不变语义**: 编辑时密钥框**留空 = 不修改**原有语义未被破坏。
- **前端接线**: 首页导航含 `#keys`、容器 `#view-keys`、脚本引用 `keys.js`、`__RICOW_TOKEN__` 已被替换; `/keys.js` `/settings.js` `/style.css` 均 200; 设置页已无密钥输入框、保留市场视野并含"前往密钥"入口; 运行页与策略页的 `need_keys` 引导已改指密钥页; 路由白名单含 `keys`; 9 个 js 全部通过 `node --check`。
- **落盘**: `ricow.toml` 权限收紧; `[[ai_key]]` / `[[exchange_key]]` 块与既有注释共存, TOML 合法。

## 四、治理同步

- `specs/architecture.md` §五 补 033 段(密钥环数据模型 + 8 条 `/api/keys*` 端点)。
- `specs/roadmap.md`: 变更档案状态加 033 注记 + 测试基线更新。
- **宪法未改**: 本变更**未新增确认渠道** —— 密钥写入属 032 已确立的 **Web 渠道普通写操作**(页面显式交互承载),
  live 逐字短语门禁一字未动, 故 `constitution.md` 版本保持 1.2.0。

## 五、偏差与遗留(如实)

1. **`ai_live_smoke` 的 2 例在本机(agent 环境)失败, 与代码无关**: `approve_requires_interactive_tty` /
   `piped_confirm_phrases_never_reach_the_host` 都只是 `spawn ricow` 并喂 stdin 管道, 报
   `Os { code: 231 }`(ERROR_PIPE_BUSY)。用一个**不含任何 ricow 代码的 20 行 Rust 程序**即可复现同一错误,
   故根因是本环境对"带 stdin 管道的子进程创建"的拦截(032 收敛时在真机终端为全绿)。
2. **ricow bin 在本机偶发抖动(已定性, 非本变更引入)**: 高负载轮次里 `ai::tools` 的部署类用例偶发
   `SQLITE_BUSY (code: 5) database is locked`, 同一份代码单跑稳定全绿、低负载轮次也全绿。根因是**测试脚手架**
   对同一个 `ricow.db` 连续开多个连接池, 而 `Database::open` 每次都跑一遍 `migrate()`(DDL = 写事务),
   上一个池靠 `Drop` 异步收尾、在 `current_thread` 运行时里可能被 Lua 编译挤后 → 下一个 open 撞上未释放的写锁。
   **本变更顺手修掉两处真实脚手架缺陷**(见 §六), 抖动显著下降但未能在此环境完全消除 —— 若要根治需让
   `Database::open` 的并发语义更强(提高 `busy_timeout` 或 schema 已最新时跳过 migrate), 那属于**产品代码语义变更**,
   不在本变更范围内, 留待单独变更决策。
3. **零测试替身**: 全程未 mock; 密钥真写进临时 `ricow.toml`、真从磁盘回读校验。
4. **未做(明确范围外)**: 密钥加密存储、轮转/到期提醒、多交易所 —— 见 spec §一"范围外"。

## 六、顺手修掉的两处既有缺陷(测试脚手架, 与 033 功能无关但同批交付)

1. **`r3_temp_root` 跨轮次复用致误报**(`ai/tools.rs`, 仅测试辅助函数): 命名只用 `pid + seq`, 而 Windows 会复用
   PID、`seq` 每轮从 0 重来, `create_dir_all` 又不清场 —— 上一轮留下的 `strategies/<name>.toml` 会让"首次部署"
   被误判成"策略已存在"。实测本机 `%TEMP%` 下堆了 **1326 个** `ricow-r3-*` 残留目录(其中 120 个 `r3-twice-*`
   含 `aidep04.toml`)。修法: 命名加单调不重复的纳秒 nonce, 并在路径万一撞上时先清场。
2. **部署类用例的连接池未确定性关闭**(同上): 在 `seed_pending_preview_with_code` 里显式
   `close_pool_for_test().await` 后再返回, 不再依赖 `Drop` 的异步收尾。
   另把 3 处断言从 `.expect("...")` 改成 `unwrap_or_else(|e| panic!("...: {e}"))` —— 原写法经 rig 的
   `ToolExecutionError` 只会打印 `model_output: "<redacted>"`, 真正的原因被吞掉(定位 §五.2 时正是卡在这里)。

## 七、未提交

代码与文档改动均在工作区, 未 `git commit`(宪法提交纪律: 用户明确说"提交"才提交)。
