# ricow

[English](README.md) | **中文**

完全本地运行的多平台量化策略引擎(回测 / Dry Run / 实盘), 纯本地 CLI 客户端工具。

> 本项目原名 **locus**, 2026-09 正式更名 **ricow**; Git 历史不继承(本仓库从 `v0.7.0` 重新开始)。

## 快速开始

### 1. 构建

```bash
cargo build --release        # 产物: target/release/ricow
```

数据目录(`$RICOW_ROOT`)解析顺序: 环境变量 `RICOW_ROOT` → 当前目录(含 `ricow.db` / `strategies/`) → 平台默认数据目录。

### 2. 配置: 只有一个文件

配置与凭据都在 `$RICOW_ROOT/ricow.toml`(权限 `0600`, 已在 `.gitignore`)。**产品不代写密钥**: 文件不存在时, 首次需要它的命令会生成带注释的模板并把路径告诉你, 你用编辑器填值即可(密钥与其供应商写在同一个 `[ai]` 段里, 不会搞混属于谁)。

```toml
[ai]                     # 可选: 只用 AI 助手时才需要
provider = "deepseek"    # 内置预设名(deepseek/moonshot/zhipu/qwen/openrouter/openai/ollama), 或任意自定义名(此时须写 base_url)
model = "deepseek-flash" # 换自建/中转端点: 再写一行 base_url = "..."
api_key=***              # 该供应商的密钥(与 provider 同段)

[exchange]               # 可选: 只在 demo / 实盘下单时才需要
demo_key=***                  # 币安测试网(demo)凭据
demo_secret=***
binance_key=***               # 币安主网凭据(真实资金)
binance_secret=***
```

**怎么拿 demo key**: 币安模拟交易(demo)平台 <https://demo.binance.com/> 有独立账号体系(与主网账号无关), 登录后在平台内创建 API Key(具体入口以官网为准; 官方说明见 <https://www.binance.com/zh-CN/support/faq/detail/ab78f9a1b8824cf0a106b4229c76496d>)。只勾**交易**权限、**不要**勾提现; 注意 demo 的 key 与旧现货测试网 `testnet.binance.vision` **不互通**。主网 key 在 <https://www.binance.com> 同样路径创建(同样建议关闭提现并限制 IP)。

### 3. 四档运行

| 档位 | 命令 | 资金 | 前置 |
|---|---|---|---|
| 回测 | `ricow backtest --strategy shannon_grid --pair ETHUSDT --days 30` | 无 | 零配置 |
| Dry Run(本地虚拟撮合) | `ricow start <名字>` | 无 | 零配置 |
| demo(币安测试网**真实下单**) | `ricow start <名字> --demo` | 模拟资金 | `[exchange].demo_*` |
| 实盘(主网真实资金) | 策略 TOML 声明 `live_enabled = true` + `ricow start <名字> --live --accept-risk` | 真实 | `[exchange].binance_*` + 首次风险确认 + Dry Run 时长门禁 |

demo 与实盘都真实调用交易所接口, 差别只在域名(`demo-api`/`demo-fapi` vs 主网)与凭据, 两套凭据**互不回落**; demo 不适用实盘的两道门(风险确认 / 时长门禁), 因为那两道门保护的是真实资金。

查看状态: `ricow list` / `ricow info <名字>`; 停机: `ricow stop <名字> [--close-all]`(按交易所侧实际结果提示残留挂单/持仓)。

### 4. AI 助手(可选)

`ricow ai "我部署了哪些策略?"` —— 用自然语言查状态、跑回测、读权威文档(全是**只读**操作); 落盘部署、启停实盘、平仓等**写实操作不在工具面内**, 只能由你本人敲命令确认。`--plain` 关闭流式输出。

### 5. 用你自己的 AI agent(可选)

你已经在用 Claude Code / Codex / Cursor 的话, 不需要 ricow 内置助手:

```
ricow agent-kit --install        # 在当前目录生成手册(也可指定目录)
ricow agent-kit                  # 只在终端打印手册
```

生成 4 个文件: `AGENTS.md`(操作手册)/ `SKILL.md`(Agent Skills 标准)/ `CLAUDE.md`(一行导入)/ `lua-api.md`(策略 API 原文)。
手册内容与内置 AI 的系统提示**同源**, 命令速查由 CLI 自己生成; 它写明写实动作(落盘/实盘/平仓/改参)**只能由你本人在终端执行**。
已存在且内容不同的文件会被**拒绝覆盖**(一个都不写), 免得冲掉你自己的 `AGENTS.md`。

### 6. 网络与数据源

ricow **全程需要访问币安**(公开行情 + 签名端点)。国内网络通常不可直连, 请自备代理/VPN —— CLI 走 `reqwest`, 认标准代理环境变量:

```
export HTTPS_PROXY=http://127.0.0.1:7890   # 改成你的代理地址(或 HTTP_PROXY / ALL_PROXY)
```

- 报 `network error` 时先确认代理是否生效: `curl https://api.binance.com/api/v3/time` 应返回 JSON。
- `RICOW_BN_BASE_URL` / `RICOW_FAPI_BASE_URL` 可整体替换 REST 域名(现货 / 合约): **公开数据与签名下单都跟着变**;
  币安官方公开数据域名 `https://data-api.binance.vision` **只提供公开数据**, 适合纯回测/看行情, **下单会失败** —— 别把它当常规解法。
- demo(`--demo`)无需手配域名: CLI 自动走 `demo-api.binance.com` / `demo-fapi.binance.com`。

### 7. 安全须知

- 凭据以**明文**存放于 `ricow.toml`(0600, 本机私有, 不入 git): 加密需要回答"解密密钥放哪"(绕回本文件=安全剧场, 绑机器指纹=换机即废)。
- 不要把密钥发给任何人(包括 AI 助手), 不要提交进仓库。
- 实盘先小额, 先跑 Dry Run 与 demo。
- 写实确认(部署 `确认部署 <名字>` / 实盘 `确认实盘 <名字>`)**只能在交互终端手动输入**: 管道、脚本、AI agent 工具调用喂入的短语一律被拒绝。

## 当前内容

- `specs/` — 唯一文档体系: constitution(项目宪法)/ product(产品方案)/ architecture(架构)/ lua-api(Lua API 规范)/ roadmap(里程碑进度)/ research(调研资料)/ changes(变更档案, SDD 流程产物)
- `crates/` — 5 crate workspace: core / binance / strategy / engine / cli
- `strategies/builtin/` — 内置参考实现(编译期嵌入二进制): shannon_grid.lua(**唯一策略样板**, 照它写你的策略)+ executors/{dca,twap,vwap,pullback,ladder}(执行模式示例, 非策略); exec 执行组件为引擎内置(Rust 实现, Lua 策略直接调用 exec.*)
- 自建策略走 `create` 闭环:`ricow create --name <名字> --pair <交易对> --script <你的.lua>` → `ricow approve` → `ricow deploy <preview_id> --token <token>`(带编译门禁 + 真实 K 线沙箱回测, **确认前不落盘**;样板 = 内置 `strategies/builtin/shannon_grid.lua`,见 [specs/lua-api.md](specs/lua-api.md) 第九节)
- `crates/ricow/src/supervisor/` — 策略进程管理器(常驻 daemon + 本机控制通道 + 实例台账, 见 [specs/architecture.md §三](specs/architecture.md))

## 平台支持

| 平台 | 状态 |
|---|---|
| Linux x86_64 | 实机验证(构建、回测、demo 测试网真实下单流程) |
| Windows x86_64 | 编译与全量单元测试已由 CI 实测通过(`test (windows-latest)`, 2026-09-15); **尚未实机跑过交易流程** |
| macOS | 未验证 —— 无实机且暂无 CI 覆盖; 代码走与 Linux 同源的 `cfg(unix)` 分支 |

## 免责声明

本软件仅供学习与研究, 不构成投资建议。加密货币交易风险极高。真实资金的任何后果由使用者自负。

---

[English README](README.md) · [项目宪法](specs/constitution.md) · [贡献指南](CONTRIBUTING.md)
