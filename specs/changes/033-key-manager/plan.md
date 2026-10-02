# 技术方案: 密钥管理页(多套 AI / 币安密钥)

**功能目录**: `specs/changes/033-key-manager`

**上游**: [spec.md](spec.md)

**创建日期**: 2026-10-02

## 一、决策清单(拍板项)

| 编号 | 决策 | 理由 / 被否方案 |
|:--|:--|:--|
| D1 | 密钥环存**同一份** `ricow.toml`, 用两个**顶层数组表** `[[ai_key]]` / `[[exchange_key]]` | 宪法「完全本地化」+ 019「密钥与其 provider 挨着、不做第二个密钥文件」。否: 独立 `keys.toml`(多一个文件要解释, 违背 019 的可理解性目标); 否 `[ai.keys."别名"]`(带引号键的行式外科编辑与错误文案都更绕) |
| D2 | `[ai]` / `[exchange]` 段**仍是唯一的"当前生效凭据"**, 字段一个不加 | 下游 `ai::config::api_key()` / `resolve()` / 运行器读凭据的路径**零改动**, 风险最小。否: 加 `active` 指针 + 读时解析(要改全部消费点, 违反"严格只改相关代码") |
| D3 | **选用 = 复制**该条写入 `[ai]` / `[exchange]`; 附带规则: 编辑"使用中"的条目后自动重新复制 | 复制带来"条目改了但生效值没跟"的隐患, 用 FR-008 的自动跟随规则消掉。代价是同一条密钥在文件里出现两次(条目 + 生效段) —— 接受, 换来运行期零改动 |
| D4 | "使用中"判定 = **值比对**(AI 比 provider/model/base_url/api_key 四元组; 币安比对应环境的 key+secret), 不落盘标记位 | 无状态、无需维护标记一致性; 手工编辑配置文件后判定依然正确 |
| D5 | 删除"使用中"条目时**同时清空**其生效字段 | 否则出现"已删除却仍生效"的幽灵凭据(用户明确要"可以删除") |
| D6 | 新增 `PUT` 语义的 `POST /api/keys/ai/save` 等**扁平端点**, 不复用 `/api/config/keys` | 后者是"外科式改单行"模型(空值=不修改), 与"整条条目"模型不同; 混在一起会让 `normalize()` 的语义分叉 |
| D7 | 块级行式编辑(定位 `[[ai_key]]` 头 + 后续键行), 保留文件其余注释 | 与 019/032 的 `upsert_line` 同一取向: 用户手写注释不能被整文件重写抹掉 |
| D8 | 前端新增 `keys.js` 视图 + `#keys` 一级导航, 「设置」页移除重复的密钥卡片 | 用户明确要求放在左侧; 两处都能配密钥必然漂移 |
| D9 | 服务商下拉来自 `ai::config::PRESETS`, 由 `GET /api/keys` 一并下发 | 复用唯一权威预设表, 前端不抄一份 |
| D10 | 不做密钥加密、不做多交易所 | YAGNI(宪法原则五); 加密存储需要主密码/系统钥匙串, 与"密钥就在 ricow.toml 里"的既有承诺冲突, 应单独立项 |

## 二、分层与改动面

```
web/assets/keys.js        新增视图(列表 + 详情 + 选用/保存/删除)
web/assets/{index.html,router.js,settings.js,style.css}   导航/白名单/去重/样式
        │  R.api
web/keyring.rs            新增: /api/keys* 的 HTTP 层(校验 + 脱敏 + 预设表)
        │  config_file::*
commands/config_file.rs   扩: 结构体 + load 解析 + 块级 upsert/remove
        │
$RICOW_ROOT/ricow.toml    [ai] / [exchange](生效)  +  [[ai_key]] / [[exchange_key]](密钥环)
```

**不改动**: `ai/config.rs`(密钥解析)、`ai/session.rs`、`supervisor/*`(运行期读凭据)、`commands/onboard.rs`(终端向导)、
`/api/config/keys`(保留给市场视野开关与旧客户端)。

## 三、数据模型

### 3.1 配置文件增量

```toml
[[ai_key]]
alias     = "工作号 DeepSeek"   # 必填, 同类内唯一, ≤24 字符
provider  = "deepseek"          # 必填(预设名或自定义名)
model     = "deepseek-flash"    # 可空: 空 = 选用后由预设推荐值兜底
base_url  = ""                  # 可空: 空 = 用预设默认
api_key   = "sk-..."            # 可空(如 ollama 无密钥)

[[exchange_key]]
alias     = "测试网"
env       = "demo"              # live | demo
key       = "bn-..."
secret    = "bn-..."
```

解析失败策略沿用 019: **拼错即硬失败并给解法**, 不静默回落。
`[[ai_key]]` 元素缺 `alias` / `provider` / `env` 非法 / 类型不对 → `CoreError::Auth` 带行级说明。

### 3.2 `GET /api/keys` 响应

```jsonc
{
  "ai": {
    "entries": [
      { "alias": "工作号 DeepSeek", "provider": "deepseek", "model": "deepseek-flash",
        "base_url": "", "api_key": { "configured": true, "hint": "***abcd" }, "active": true }
    ],
    "current": { "provider": "deepseek", "model": "deepseek-flash", "base_url": "",
                 "api_key": { "configured": true, "hint": "***abcd" },
                 "from_entry": "工作号 DeepSeek" }        // 未来自条目 → null
  },
  "exchange": {
    "entries": [
      { "alias": "测试网", "env": "demo",
        "key": { "configured": true, "hint": "***9988" },
        "secret": { "configured": true, "hint": "***7777" }, "active": true }
    ],
    "current": { "binance_key": {...}, "binance_secret": {...},
                 "demo_key": {...}, "demo_secret": {...},
                 "live_from": null, "demo_from": "测试网" }
  },
  "presets": [ { "id": "deepseek", "label": "DeepSeek 推荐",
                 "base_url": "https://api.deepseek.com/v1", "model": "deepseek-flash" } ]
}
```

### 3.3 端点

| 方法 | 路径 | 体 | 语义 |
|:--|:--|:--|:--|
| GET | `/api/keys` | — | 密钥环总览(全脱敏) |
| POST | `/api/keys/ai/save` | `{alias, prev_alias?, provider, model, base_url, api_key?}` | 新增或更新 AI 条目; `prev_alias` 用于改名 |
| POST | `/api/keys/ai/use` | `{alias}` | 选用: 复制该条到 `[ai]` |
| POST | `/api/keys/ai/delete` | `{alias}` | 删除条目(+ 若是使用中则清空生效字段) |
| POST | `/api/keys/exchange/save` | `{alias, prev_alias?, env, key?, secret?}` | 新增或更新币安条目 |
| POST | `/api/keys/exchange/use` | `{alias}` | 选用: 复制到 `[exchange]` 对应环境 |
| POST | `/api/keys/exchange/delete` | `{alias}` | 删除条目(+ 幽灵凭据防护) |
| POST | `/api/keys/clear` | `{target}` | 清空生效凭据; `target` ∈ `ai` / `live` / `demo` |

全部成功回 `204 No Content`(与 `/api/config/keys` 一致), 失败回统一 `WebError` JSON:
400 + 中文原因(别名重复 → `code:"dup_alias"`; 别名非法 → `code:"invalid_alias"`; 未知别名 → 404)。
密钥空值语义: 体里 `api_key` / `key` / `secret` **缺省或空串 = 保持原值**, "删除密钥"不是清空字段而是**删条目/清除生效**。

## 四、前端设计

- 视图 `R.views.keys`, hash `#keys`, 导航插在「运行」与「设置」之间。
- 布局: 上下两组(「AI 通道」/「币安凭据」), 每组 = 左列条目列表 + 右列详情表单 + 顶部"当前生效"摘要条。
- 条目行: 别名(粗) + 次要信息(服务商 / 环境) + 尾号 + `使用中` 徽标。
- 详情表单: 别名/服务商(预设下拉 + `自定义…`)/模型/接口地址/API Key; 按钮 `选用这套` / `保存` / `删除`。
  - 服务商选预设时把预设的 model/base_url 作为 placeholder, 不强行写入值(避免替用户做决定)。
- 删除前 `confirm()` 二次确认; 弹窗文案明确"若正在使用中, 生效凭据会一并清空"。
- 与 032 同一约束: 不写任何浏览器持久存储; 密钥框 `type="password"`, 留空 = 不修改。

## 五、测试策略

**单元测试(纯逻辑, 不碰网络)**:

1. `config_file`: 块级 `upsert`/`remove` 的行式编辑(新增/改名/替换/删除后**其余注释与内容逐字不变**、重复别名被拒、CRLF 保持);
2. `config_file`: `load` 解析 `[[ai_key]]` / `[[exchange_key]]`, 缺字段与非法 `env` 硬失败;
3. `keyring`: 别名校验(空/超长/控制字符/重复)、"使用中"判定(四元组比对)、`hint` 三态、
   `GET` 响应**断言不含明文**、删除使用中条目后生效字段被清空;
4. `web/mod.rs`: 新增端点的鉴权覆盖(无 token → 401)。

**真机冒烟(testnet / 本机 Web)**: 按 spec §二 的三个用户故事逐条跑:`/api/keys` 读写 → 落盘文件核对 →
`[ai]` 段生效核对 → 启动 web 服务用 curl 走一遍; 密钥为**占位值**(不引入真实密钥), 网络类调用不涉及。

**不做的测试**: 不新增 mock; 密钥解析已有路径由既有测试覆盖(SC-004)。
