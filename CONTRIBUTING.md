# 贡献指南(Contributing)

> 本文档以中文为主(与 `specs/` 文档体系、提交信息一致)。英文概览见 [README.md](README.md)。
> This guide is written in Chinese, consistent with the project's `specs/` docs and commit messages. See [README.md](README.md) for an English overview.

感谢你的贡献。提交前请先读 [项目宪法](specs/constitution.md) —— 它是本仓库的**唯一权威**,本文只是它的操作化说明。

## 一、环境准备

| 项 | 要求 |
|---|---|
| Rust | **版本由 `rust-toolchain.toml` 钉死**(当前 `1.96.1`, 含 rustfmt/clippy 组件) —— 本地与 CI 必须一致, 否则 clippy 门禁会因 upstream 发版而随机变红;升级工具链是**独立变更**(改 `rust-toolchain.toml` + 清掉新告警 + 实测);`Cargo.toml` 声明 MSRV `rust-version = 1.88` |
| 系统 | Linux / macOS / Windows 均可构建;Linux x86_64 是唯一实机验证过的平台 |
| 网络 | 回测/demo/实盘**都要访问币安**;国内需自备代理(`HTTPS_PROXY`) |

```bash
git clone git@github.com:ailenwu2000/ricow.git
cd ricow
cargo build --release          # 产物 target/release/ricow
cargo test --workspace         # 纯逻辑单元测试, 零配置(不需要任何密钥)
```

## 二、测试纪律(不可协商)

见宪法原则三。要点:

- **交易流程(下单 / 撤单 / 成交 / 持仓 / 风控)必须用真实 testnet 调用**(币安 demo:`demo-api.binance.com` / `demo-fapi.binance.com`)。
- **禁止** mock 交易所替身、禁止假 token、禁止在主网做测试下单。
- 纯逻辑(指标 / 打分 / 参数校验)用单元测试(已知向量),这不属于 mock。

仓库里需要真实外部环境的用例标了 `#[ignore]`,CI **不会**跑它们。维护者本地跑法:

```bash
export RICOW_BN_API_KEY=...  RICOW_BN_SECRET_KEY=...     # demo 平台凭据, 只勾交易权限
cargo test -p ricow_binance -- --ignored --test-threads=1
```

改动触及交易路径的 PR,请在描述里写明「已在 demo 环境实测」或明确写「未实测,原因」——我们宁可看到诚实的「未验证」,也不要看到编造的绿色。

## 三、开发流程(SDD)

本项目用 spec-kit 的 SDD 流程,所有非平凡变更走:

`/speckit-specify`(规格)→ `/speckit-plan`(技术方案)→ `/speckit-tasks`(任务分解)→ `/speckit-implement`(实施)→ `/speckit-converge`(对照收敛)

- 产物落 `specs/changes/<feature>/`(spec.md / plan.md / tasks.md,收敛后加 converge.md),**不要**把计划留在临时目录。
- 小改动(typo / 文档同步)可直接提 PR,不必走全流程。
- 影响系统级行为的变更,收尾时必须同步复核 `specs/architecture.md` / `lua-api.md` / `backtest.md` 里受影响章节与测试基线数字。

## 四、分支、提交与 PR

- 分支模型:**GitHub Flow**。`main` 是唯一长期分支;每个改动开短命分支:`feat/<简述>`、`fix/<简述>`、`docs/<简述>`。
- 提交信息:中文,`type(scope): 描述`,类型沿用 `feat / fix / docs / chore / refactor / test / perf`。示例:`fix(engine): 修复 hedge 空方向单计费`。
- PR 要求:
  - CI 全绿(lint + 测试);
  - 不含任何密钥/凭据/个人数据(`ricow.toml`、`ricow.db*` 已在 `.gitignore`,别用 `-f` 加进来);
  - 影响交易路径 → 写明 demo 实测结论(见第二节);
  - 影响文档承诺 → 同 PR 内同步改 `specs/`;
  - 描述模板里的自检清单逐条勾选。
- 合并方式:**Squash merge**(线性历史,便于回滚与 `git log` 阅读)。合并后源分支会自动删除。
- 保持分支短命(建议 1-3 天),长期分支会积累冲突与集成风险。

## 五、代码风格

- 格式:`cargo fmt --all`(遵循 `rustfmt.toml`:`max_width = 100`,`use_small_heuristics = "Max"`),**CI 已强制 `cargo fmt --all -- --check`**(2026-09-15 全仓对齐)。
  提交前跑一次 `cargo fmt --all` 即可;工具链版本由 `rust-toolchain.toml` 钉死,保证格式化结果可复现。
- 静态检查:`cargo clippy --workspace --all-targets -- -D warnings` —— **CI 为硬失败**(2026-09-15 存量告警已清零),任何新告警都会挡住合并。
- 注释:中文(技术名词与命令保留英文)。
- 依赖:新增依赖前先说明理由(宪法原则五「少而精」);能用现有接口/数据结构解决的,不引入新机制。

## 六、安全

- 发现漏洞请走 [SECURITY.md](SECURITY.md) 的私有渠道,**不要在公开 issue 里贴密钥或可利用细节**。
- 任何提交进仓库的密钥都视为已泄露:立刻在交易所/供应商侧吊销并轮换,再开 PR 清理历史。
- 提交前可用 `git check-ignore -v ricow.toml ricow.db` 确认本地机密文件仍被忽略。

## 七、语言约定

| 内容 | 语言 |
|---|---|
| `specs/` 文档、代码注释、提交信息、PR/issue 讨论 | 中文 |
| `README.md` | 英文(面向国际读者) |
| `README_zh.md` | 中文 |
| 技术名词、命令、API 名 | 保留英文 |

## 八、项目组成员(Write 权限)与上手清单

**权限模型**(2026-09-15 决定):

- 仓库 <https://github.com/ailenwu2000/ricow> 是个人仓库;**未加入的人**只能 fork 后提 PR,合并需维护者同意。
- 项目组成员由维护者在 `Settings → Access → Add people` 加入(填 GitHub 用户名或已绑定该账号的邮箱),拿到 **Write** 权限。
- **成员可直接推 `main`**(本项目当前约定)。但直推不等于免检:
  - CI 四个门禁必须绿 —— `rustfmt` / `clippy -D warnings` / `test (ubuntu-latest)` / `test (windows-latest)`;推红了要立刻修(CI 在推送后依然会跑并如实报红);
  - 不得推入任何密钥/凭据/个人数据(见第六节);
  - 触及交易路径的改动,PR 描述或提交信息里写明 demo 实测结论;
  - 影响 `specs/` 承诺的改动,同一提交内同步文档。
- 与他人协作的功能改动**建议仍走 PR**(便于评审与回溯);直推适合小修、文档、紧急修复。合并方式 = Squash merge。

**新成员上手清单**(约 10 分钟):

```bash
git clone git@github.com:ailenwu2000/ricow.git && cd ricow
cargo test --workspace                                    # 纯逻辑测试, 零配置
cargo build --release && ./target/release/ricow --help     # 产物与命令面
./target/release/ricow backtest --strategy shannon_spot_grid --pair ETHUSDT --days 20   # 需能访问币安(国内配 HTTPS_PROXY)
```

1. 读 [项目宪法](specs/constitution.md)(唯一权威)与本文;
2. 工具链无需手动装 —— `rust-toolchain.toml` 钉死 `1.96.1`,首次 `cargo` 命令会自动安装;
3. 要跑 demo/实盘联调:自备币安 demo 平台凭据(只勾交易权限),按第二节的 `#[ignore]` 用法本地跑,**不要**把密钥写进仓库或 CI;
4. 改动走第四节的 GitHub Flow;不确定的先开 issue 讨论。

## 九、行为准则

参与本项目即表示你同意遵守 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。
