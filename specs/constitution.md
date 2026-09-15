# ricow 项目宪法

**版本**: 1.1.0 | **批准日期**: 2026-09-01 | **最后修订**: 2026-09-15

> 修订记录:
> - 1.0.1 (2026-09-12) — 原则三测试纪律去 HL(移除 Hyperliquid 适配, 见 `specs/changes/009-remove-hyperliquid/`)。
> - 1.1.0 (2026-09-15) — **项目更名 locus -> ricow**(仓库/二进制/crate/环境变量/配置与数据文件名统一; git 历史不继承, 见 `specs/roadmap.md` 更名注记);
>   原则四补充"根级对外文档"的语言分工; 文档体系承认仓根开源文件与 `.github/` 的职责; 新增「分支与发布」章。

## 核心原则

### 一、完全本地化
私钥不出本机; 行情/策略/密钥/数据全在用户机器上; 无云端依赖、无遥测、无自动更新。
网络仅访问交易所 API 与用户自配的 LLM API。

### 二、策略层统一 Lua + exec 组件
策略 = 信号逻辑 + 调 `exec.*` 执行(Rust 实现, 引擎内置, 脚本内可覆盖)。
内置资产(`strategies/builtin/`)编译期嵌入二进制; 用户策略复制即自定义。
DCA/TWAP/限价回调/阶梯是**执行算法**, 不是独立策略。

### 三、测试纪律(不可协商)
交易流程(下单/撤单/成交/持仓/风控)必须用 testnet 真实调用:
BN 币安 demo 测试网(`demo-api.binance.com` 现货 / `demo-fapi.binance.com` 合约, 见 `specs/testnet.md`); 禁 mock Exchange 替身、禁假 token、禁主网下单测试。
纯逻辑(指标/打分/参数校验)用单元测试(已知向量), 不属 mock。

### 四、产物一律中文
本仓库所有 spec / plan / tasks / checklist / roadmap 文档一律中文。
代码注释与提交信息中文; 英文仅保留技术名词与命令。
**例外(2026-09-15 增补)**: 面向仓库外读者的根级文件可用英文或双语 —— `README.md` 英文(对外门面)、`README_zh.md` 中文;
`CONTRIBUTING.md` / `SECURITY.md` / `CODE_OF_CONDUCT.md` / `.github/` 下的模板以中文为主(可附英文摘要)。
spec-kit 命令模板保持官方英文逻辑, 产物由本宪法与 zh preset 保证中文。

### 五、少而精(YAGNI)
只做必要功能; 不做多余的事情; 架构参数基于数据源实测精确设定, 不留无依据的保守余量;
拒绝过度设计; 死代码必删不留回退。

## 对话与变更纪律

- 实事求是: 用户提议不好时直接拒绝并给更优方案; 用户确认要做某件事时会说明原因, 不完全确认时以讨论为主不修改文件。
- 代码修改前先出审查计划再执行, 严格只改相关代码, 不顺手重构。
- 部署/系统级操作与破坏性删除由用户自执行, agent 只给步骤不代跑。
- git 提交: 用户明确说"提交"才可 commit, 严禁主动提交。
- 问题排查: 线上/运行时问题先查根因(配置/环境/日志)再决定是否改代码。

## 文档体系(唯一权威)

- `specs/` 是**唯一文档树**: constitution(本文件)/ product / architecture / backtest(回测规范)/ lua-api / roadmap 为系统级 spec(living spec, 小更新直接演进, 大变更走变更流程); `specs/testnet.md` 为测试网环境与凭据记录(仅测试网, 已授权入库); `specs/research/` 为调研档案(证据与结论留档, 已定稿不回改); `specs/changes/<feature>/` 为变更档案。
- 变更档案须齐 spec.md / plan.md / tasks.md, 收敛后加 converge.md; 计划产物不留在 `specs/` 之外的临时目录(2026-09-11 澄清, 因 005/006/007 计划曾只落在 `.hermes/plans/`)。
- 系统级 spec 的"现状描述"必须与代码一致: 每次变更收尾(converge)时同步复核 architecture / lua-api / backtest 中受影响章节与测试基线数字。
- `AGENTS.md` 仅为 Hermes 入口指针, 内容单一来源 = 本文件。
- **仓根开源文件(2026-09-15 增补)**: `README.md` / `README_zh.md` / `CONTRIBUTING.md` / `SECURITY.md` / `CODE_OF_CONDUCT.md` / `LICENSE` / `.github/`(CI、issue/PR 模板、CODEOWNERS、dependabot)属协作与合规必需文件, 不归 `specs/` 管;
  发布流程与平台矩阵的**唯一权威**是 `specs/release.md`。
- 进度与里程碑唯一权威 = `specs/roadmap.md`; 改进度只改该文件一处。
- 历史(CHANGELOG)后置到 P5 公开发布时再建, 发布前不建空壳。

## 开发流程

- 所有变更走 SDD: `/speckit-specify`(规格)→ `/speckit-plan`(技术方案)→ `/speckit-tasks`(任务分解)→ `/speckit-implement`(实施)→ `/speckit-converge`(对照收敛, 不收敛继续)。
- 计划先审后执行: 方案未定不写实现; 决策用编号清单(D1/D2…)拍板; 审核按严重度分级。
- 产物文件: spec.md / plan.md / tasks.md 存档于 `specs/changes/<feature>/`, 收敛后归档。

## 安全要求

- 写操作强制确认: 提交 → 编译门禁 → 沙箱回测 → preview → `ricow approve`(一次性 token 重放) → 部署。
- RiskEngine 硬检查(最大持仓 / 单日最大亏损 / 最小订单 / 最大滑点)。
- Dry Run 默认, 确认后切实盘。
- Lua 沙箱: 无 os/io/require/loadstring/pcall; 指令预算 1M/tick; 内存 64MB。

## 分支与发布(2026-09-15)

- **分支模型**: GitHub Flow —— `main` 为唯一长期分支 + 短命功能分支(`feat/` `fix/` `docs/`)+ PR; 不引入 develop / release 分支。
- **提交与合并**: 提交信息中文 `type(scope): 描述`; 合并用 squash。项目组成员可直接向 `main` 推送(2026-09-15 决定); 外部贡献者经 fork + PR, 由维护者批准后合并。
- **版本与发布**: 语义化版本; 0.x 期间破坏性改动升 minor。发布 = 改 `[workspace.package] version` -> 提交 -> 推 `vX.Y.Z` tag -> CI 自动多平台构建并挂 GitHub Release; **禁止移动/改写已发布 tag**, 修正一律发新版本。
- **产物平台**: 以 CI 实测结果为准, 未经验证的平台**不得**在 README / Release 说明里宣称支持; 详见 `specs/release.md`。
- **历史**: 本仓库 git 历史自 2026-09-15 起全新开始(旧 locus 历史不继承); 历史变更档案保留旧名, 不回改。

## 治理

- 本宪法优先于其他一切实践; 修订需记录、批准、迁移计划。
- 所有变更必须符合本宪法; 复杂度必须被证明必要。
- 任何文档变更本身也是变更, 走上述流程(小更新可直接演进 living spec)。
