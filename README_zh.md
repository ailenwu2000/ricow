# ricow · 富牛

<img src="website/logo.svg" alt="ricow 富牛" width="96">

[English](README.md) | **中文**

> 中文名:**富牛**。

完全本地运行的多平台量化策略引擎(回测 / Dry Run / 实盘), 纯本地 CLI 客户端工具。

**从灵感，到规则，到实盘。**

> 本项目原名 **locus**, 2026-09 正式更名 **ricow**; Git 历史不继承(本仓库从 `v0.7.0` 重新开始)。

## 安装

预编译的自包含产物发在 [releases 页面](https://github.com/ailenwu2000/ricow/releases), **不需要 Rust 工具链**。同一份配置生成四类安装方式: shell、PowerShell、Homebrew、msi。

**Linux / macOS —— shell 安装脚本**

```bash
curl -LsSf https://github.com/ailenwu2000/ricow/releases/latest/download/ricow-installer.sh | sh
```

**Windows —— PowerShell 安装脚本**

```powershell
irm https://github.com/ailenwu2000/ricow/releases/latest/download/ricow-installer.ps1 | iex
```

**Windows —— msi**

从 releases 页面下载 `ricow-x86_64-pc-windows-msvc.msi` 双击安装。它把 `ricow.exe` 装到 `%ProgramFiles%\ricow\bin` 并追加进 `PATH` —— 请**新开**一个终端让改动生效。msi 只含可执行文件; 下面那个双击入口在 `.zip` 里。

**macOS —— Homebrew**

```bash
curl -LO https://github.com/ailenwu2000/ricow/releases/latest/download/ricow.rb
brew install ./ricow.rb
```

目前**没有 tap**, 所以 `brew install ricow` 用不了: 我们把公式本身作为 release 产物发布, 它直接装预编译二进制(不编译)。

**任意平台 —— 手动解压**

下载对应平台的压缩包, 解压后运行 `./ricow`(Windows 是 `ricow.exe`)。每个压缩包都带内置助手的启动脚本: Windows 双击 `启动-ricow-AI助手.cmd`(它替你切到 UTF-8 —— 中文不乱码就是靠这一步); Linux/macOS 执行 `./启动-ricow-AI助手.sh`; macOS 上也可直接双击 `启动-ricow-AI助手.command`, 它只是同一个脚本的双击外壳。启动脚本走的是 ricow 的裸入口, 所以第一次启动会在**对话里**问你要供应商与 API Key(输入不回显), 写进 `ricow.toml` 之后直接进入会话 —— 事先不需要设任何环境变量。想用网页版就双击同目录的 `启动-ricow-Web.cmd` / `启动-ricow-Web.command`(Linux/macOS 执行 `./启动-ricow-Web.sh`): 它会起本机网页并自动打开浏览器, 见 §5。

**升级方式。** 我们发布的产物里**不含自动更新组件**: 程序不会从网络改写自己, 升级是你主动的动作 —— 下载新的压缩包/msi, 或重跑安装脚本。**不提供** winget / scoop 包; 请用 PowerShell 脚本、msi 或 `.zip`。

## 快速开始

### 1. 从源码构建(可选 —— 已装 release 就跳过)

```bash
cargo build --release        # 产物: target/release/ricow
```

数据目录(`$RICOW_ROOT`)解析顺序: 环境变量 `RICOW_ROOT` → 当前目录(含 `ricow.db` / `strategies/`) → 平台默认数据目录。

在源码检出里启动助手, 与压缩包里的方式完全一致: Windows 双击 `packaging\启动-ricow-AI助手.cmd`, Linux/macOS 执行 `packaging/启动-ricow-AI助手.sh`。它按「脚本同目录 → `target/`(release 与 debug 中较新的那个)→ `PATH`」查找二进制; 回落到 `target/` 时会先 `cd` 到仓库根, 因此助手用的是本检出里的 `strategies/`, 而不是另建一个空数据目录。`cargo ai` 是本仓库的 alias(见 `.cargo/config.toml`, 等价于 `cargo run -p ricow -- ai`)—— `ai` 是 ricow 的子命令而非 cargo 子命令, 所以这种写法只在检出内存在。网页版同理: 双击 `packaging\启动-ricow-Web.cmd` / 执行 `packaging/启动-ricow-Web.sh`, 或直接 `cargo run -p ricow -- web`。

### 2. 配置: 只有一个文件

配置与凭据都在 `$RICOW_ROOT/ricow.toml`(已入 `.gitignore`;Unix 下写为 `0600` —— Windows 没有 POSIX 权限位, 那边靠写入时收紧 ACL 到仅当前用户)。文件不存在时, 首次需要它的命令会生成带注释的模板并把路径告诉你, 你用编辑器填值即可 —— 或者在 `ricow`(见 §4)里用 `/keys`(掩码输入, 不回显)与 `/market` 改; 这两条命令**按行外科式改写, 保留你的注释**, 只碰 9 个白名单键。密钥与其供应商写在同一个段里, 不会搞混属于谁。

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

`ricow ai "我部署了哪些策略?"` —— 用自然语言查状态、跑回测、读权威文档(全是**只读**操作)。裸 `ricow` 进入同一助手的交互会话(`chat`); 全新安装时会先走首次配置向导(可选中文 / 英文)。落盘部署、覆盖、改参数、删除、启停试跑与测试网、实盘全部动作等**写操作不在工具面内**: 模型只能登记一条待确认动作, 由**你本人确认**后宿主进程才执行 —— 对话里回一个当前语言的口语词即可(`确认`/`确定`/`同意` · `confirm`/`confirmed`), 终端命令才需要逐字长短语(如 `确认实盘 <名字>`)。斜杠命令: `/help` `/history` `/lang` `/keys` `/market` `/exit`。`--plain` 关闭流式输出。

### 5. Web UI(可选)

`ricow web` —— 在你本机 `127.0.0.1` 上起一个内置网页(端口随机), 启动时打印带一次性 token 的网址并尝试自动打开浏览器, 之后所有对话与操作都在页面里完成。**服务端与 CLI 是同一个会话引擎**, 只是换了前端: 左侧是会话历史(可新建/切换/删除, 重开旧会话带上最近 20 轮作上下文), 右侧上为对话输出区、下为输入区; 关键字/警告/出错按级别着色, 量化术语点击弹解释; 界面可中英切换(与 CLI 共用 `ricow.toml` 的 `[ui].lang`)。token 每次启动随机生成、**不落盘不进日志**, 页面与所有接口都在 token 门禁之后(无 token 或错 token 一律 `401` 且不回任何会话内容)—— 换句话说**它只给本机用**, 不暴露到局域网或公网。

压缩包里也带双击入口: Windows 双击 `启动-ricow-Web.cmd`, macOS 双击 `启动-ricow-Web.command`(或执行 `./启动-ricow-Web.sh`), Linux 执行 `./启动-ricow-Web.sh` —— 与 AI 助手那三个脚本同一套查找逻辑, 只是把启动行换成 `ricow web`。

### 6. 用你自己的 AI agent(可选)

你已经在用 Claude Code / Codex / Cursor 的话, 不需要 ricow 内置助手:

```
ricow agent-kit --install        # 在当前目录生成手册(也可指定目录)
ricow agent-kit                  # 只在终端打印手册
```

生成 4 个文件: `AGENTS.md`(操作手册)/ `SKILL.md`(Agent Skills 标准)/ `CLAUDE.md`(一行导入)/ `lua-api.md`(策略 API 原文)。
手册内容与内置 AI 的系统提示**同源**, 命令速查由 CLI 自己生成; 它写明写实动作(落盘/实盘/平仓/改参)**只能由你本人在终端执行**。
已存在且内容不同的文件会被**拒绝覆盖**(一个都不写), 免得冲掉你自己的 `AGENTS.md`。

### 7. 网络与数据源

ricow **全程需要访问币安**(公开行情 + 签名端点)。国内网络通常不可直连, 请自备代理/VPN —— CLI 走 `reqwest`, 认标准代理环境变量:

```
export HTTPS_PROXY=http://127.0.0.1:7890   # 改成你的代理地址(或 HTTP_PROXY / ALL_PROXY)
```

- 报 `network error` 时先确认代理是否生效: `curl https://api.binance.com/api/v3/time` 应返回 JSON。
- `RICOW_BN_BASE_URL` / `RICOW_FAPI_BASE_URL` 可整体替换 REST 域名(现货 / 合约): **公开数据与签名下单都跟着变**;
  币安官方公开数据域名 `https://data-api.binance.vision` **只提供公开数据**, 适合纯回测/看行情, **下单会失败** —— 别把它当常规解法。
- demo(`--demo`)无需手配域名: CLI 自动走 `demo-api.binance.com` / `demo-fapi.binance.com`。
- **内置数据源**(`ricow data pull --source <名>`): `binance_spot` / `binance_futures`(区间 K 线, 自动翻页)与 `nasdaq` / `yahoo`(美股日线, 免 key)。全部落进同一张本地表 `data_klines`, 键 = `(source, symbol, interval, open_time)`。
- **回测只读本地库**(可复现): 先 `ricow data pull --source binance_spot --symbol ETHUSDT --interval 1h --days 150`, 再 `ricow backtest …`; 缺数据时错误里直接印出该敲的 `data pull` 命令, 回测中途不会偷偷联网。
- **你的策略可以自己取数**: Lua 策略用 `http:get(url)`(仅 GET, 墙钟超时 10s, 响应体上限 5MB)。域名不限 —— 风险由你自担(平台不为这些请求背书); 项目自带策略一律不用它。

### 8. 安全须知

- 凭据以**明文**存放于 `ricow.toml`(本机私有, 不入 git; Unix 为 `0600`, Windows 为仅当前用户的 ACL): 加密需要回答"解密密钥放哪"(绕回本文件=安全剧场, 绑机器指纹=换机即废)。
- 不要把密钥发给任何人(包括 AI 助手), 不要提交进仓库。
- 实盘先小额, 先跑 Dry Run 与 demo。
- 写实确认由**你本人在交互终端逐字输入** —— 部署 `确认部署 <名字>` / 启动测试网 `确认启动测试网 <名字>` / 首次实盘风险确认 `确认风险` / 实盘 `确认实盘 <名字>` / 停止测试网 `确认停止测试网 <名字>` / 停止实盘 `确认停止实盘 <名字>` / 平仓并停止 `确认平仓停止 <名字>`。管道、脚本、AI agent 工具调用喂入的短语一律被拒绝, 裸 `y`/`yes`/回车也不算。实盘仍会原样重跑宿主三门禁(风险确认 → Dry Run 时长 → 时钟预检)。

## 常见问题

**Windows 控制台里中文是乱码/方块。** 控制台代码页不是 UTF-8。先在该终端执行 `chcp 65001`, 或者用 `启动-ricow-AI助手.cmd` 启动(它替你做了这一步 —— 这个文件存在的全部理由)。想一劳永逸: 打开 Windows 11 的*设置 → 时间和语言 → 语言和区域 → 管理语言设置 → Beta: 使用 Unicode UTF-8 提供全球语言支持*。

**Windows 报「Windows 已保护你的电脑」/ 被 SmartScreen 拦下。** 我们没有代码签名证书(这是有意的决定, 见下面两条)。点*更多信息 → 仍要运行*; 或对解压出来的文件做一次性解锁, 从此不再弹:

```powershell
powershell -Command "Get-ChildItem -Recurse .\ | Unblock-File"
```

**macOS 报「无法打开, 因为无法验证开发者」(或「已损坏」)。** 这是 Gatekeeper, 不是下载坏了: 二进制既没签名也没公证。去掉隔离标记即可:

```bash
xattr -d com.apple.quarantine ./ricow
```

或用*系统设置 → 隐私与安全性 → 仍要打开*。**为什么不签名**: 苹果开发者证书是付费订阅, Windows 证书需要企业主体; 而本项目的立场是「自己构建、只信自己读过的源码」—— `cargo build --release` 得到的二进制不需要任何人背书。

**Linux/macOS 上启动脚本报 `Permission denied`。** 双击或执行启动脚本需要可执行位; 压缩包一般带着它, 万一丢了就补一次: `chmod +x ricow 启动-ricow-AI助手.sh 启动-ricow-AI助手.command`(脚本自己也会在检测到 `./ricow` 存在但不可执行时给出这条提示)。

**怎么换 AI 供应商, 或用本地模型?** 改 `ricow.toml` 里的 `[ai]`: `provider`(预设: `deepseek`(默认)/ `moonshot` / `zhipu` / `qwen` / `openrouter` / `openai` / `ollama`)、`model`, 不在列表里的再补 `base_url`。该供应商的 `api_key` 写在同一个段里。在助手会话里用 `/keys` 可以把 provider + 密钥 + 模型写回文件, 且保留你的注释。

**能完全离线跑吗?** AI 那半边可以: `provider = "ollama"` + `base_url = "http://127.0.0.1:11434/v1"` + 你本地已拉取的模型(`ollama list`), 全程不联网。**回测也能离线**: 数据先 `ricow data pull` 落到本地库, 之后回测只读本地库、不联网(这正是可复现的前提)。demo/实盘要真实下单, 所以币安必须可达(国内网络请设 `HTTPS_PROXY`)。只有 AI 端点这一项是可选的。

**策略、数据库、配置放在哪?** 都在 `$RICOW_ROOT` —— 解析顺序: 环境变量 `RICOW_ROOT` → 当前目录(已经含 `ricow.db`/`strategies/` 时) → 平台默认数据目录。`ricow.toml`(Unix `0600` / Windows 仅当前用户 ACL)与 `ricow.db` 都在那里。

**包多大、编译要多久?** 2026-09-17 在维护者机器上实测(如实记录, 非预判): `dist` profile 的 `ricow.exe` 为 21,042,176 字节(20.1 MiB), `ricow.exe` + `README.md` + `LICENSE` 打成的 Windows `.zip` 为 8,614,204 字节(≈8.2 MiB); 该 profile 从零冷编译耗时 284.5 秒(4m44s)。普通 `cargo build --release` 产物为 19,930,624 字节(≈19.0 MiB)。一台机器一次测量 —— 请当数量级参考, 不是承诺。

## 当前内容

- `specs/` — 唯一文档体系: constitution(项目宪法)/ product(产品方案)/ architecture(架构)/ lua-api(Lua API 规范)/ roadmap(里程碑进度)/ research(调研资料)/ changes(变更档案, SDD 流程产物)
- `crates/` — 5 crate workspace: core / binance / strategy / engine / cli
- `strategies/builtin/` — 内置参考实现(编译期嵌入二进制): shannon_grid.lua(策略样板, 照它写你的策略)+ executors/{dca,twap,vwap,pullback,ladder}(执行模式示例, 非策略); exec 执行组件为引擎内置(Rust 实现, Lua 策略直接调用 exec.*)
- `strategies/examples/ema_cross_declared.lua` — **声明式数据面样板(028)**: `data:series{source,symbol,interval,bars,min_bars,drive}` + `on_bar` + 句柄指标(`s:ema`), 策略自己决定来源/标的/周期
- 自建策略走 `create` 闭环:`ricow create --name <名字> --pair <交易对> --script <你的.lua>` → `ricow approve` → `ricow deploy <preview_id> --token <token>`(带编译门禁 + 真实 K 线沙箱回测, **确认前不落盘**;样板 = 内置 `strategies/builtin/shannon_grid.lua`,见 [specs/lua-api.md](specs/lua-api.md) 第九节)
- `website/` — 官网落地页(<https://ricow.xyz>,纯静态 HTML/CSS,由 `.github/workflows/pages.yml` 部署到 GitHub Pages)
- `crates/ricow/src/supervisor/` — 策略进程管理器(常驻 daemon + 本机控制通道 + 实例台账, 见 [specs/architecture.md §三](specs/architecture.md))

## 平台支持

| 平台 | 状态 |
|---|---|
| Linux x86_64 | 实机验证(构建、回测、demo 测试网真实下单流程) |
| Linux arm64 | 由 CI 构建并发布; 尚未实机跑过交易流程 |
| Windows x86_64 | 编译与全量单元测试已由 CI 实测通过(`test (windows-latest)`, 2026-09-15); **尚未实机跑过交易流程** |
| macOS | 未验证 —— 无实机且暂无 CI 覆盖; 代码走与 Linux 同源的 `cfg(unix)` 分支 |

四类安装方式由同一份配置生成, 但只有 shell 脚本在实机(Linux)上跑过; PowerShell 安装器、msi 与 Homebrew 公式**尚未**在真实 Windows/macOS 机器上装过。本地**已核对**的是: `dist plan` 列出 shell + PowerShell + Homebrew + msi 四类, 每个压缩包都带 `LICENSE`/`README.md`/`启动-ricow-AI助手.cmd`/`启动-ricow-AI助手.sh`/`启动-ricow-AI助手.command`, 且产物中没有任何更新器。

## 免责声明

本软件仅供学习与研究, 不构成投资建议。加密货币交易风险极高。真实资金的任何后果由使用者自负。

---

[English README](README.md) · [项目宪法](specs/constitution.md) · [贡献指南](CONTRIBUTING.md)

### 网络受限环境

取数走标准环境变量代理(平台不提供代理配置项):

```bash
HTTPS_PROXY=http://127.0.0.1:1080 ricow data pull --source yahoo --symbol QQQ --interval 1d --days 3650
```

回测只读本地库, 不需要网络。**取数失败一律如实报错**(不降级、不静默改用旧数据); 受限网络下请给取数命令配上代理环境变量(如上)。
