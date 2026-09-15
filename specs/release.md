# 发布规范(release)

> 本文是**发布流程与产物平台矩阵的唯一权威**。宪法「分支与发布」章为原则, 本文为可执行清单。
> 状态: 2026-09-15 建立; 同日接入多平台构建工具 dist(cargo-dist 0.32.0), 见 §四。

## 一、版本规则

- 遵循语义化版本(SemVer): `MAJOR.MINOR.PATCH`, tag 形式 `vX.Y.Z`。
- 0.x 期间: 破坏性/行为变更升 **minor**(`0.7.0 → 0.8.0`), 修复升 patch。达到稳定后再谈 1.0。
- **单一版本源**: 根 `Cargo.toml` 的 `[workspace.package] version`; 5 个 crate 均 `version.workspace = true`, 改一处即全仓一致。
- **禁止移动或改写已发布的 tag**(及 Release 资产)。发现缺陷一律发新版本(`vX.Y.(Z+1)`), 不改历史。

## 二、发布前检查(逐条)

1. `cargo build --release --locked` 与 `cargo test --workspace --locked` 全绿;
2. 涉及交易路径的改动: 维护者在币安 demo 测试网真实调用验证(`#[ignore]` 用例, 见 `specs/testnet.md`), 并把结果追加到该文件;
3. `specs/` 受影响章节与测试基线数字已同步(宪法 §文档体系);
4. 无密钥/凭据/个人数据进入提交(`git status` 复核; `ricow.toml`、`ricow.db*` 已被 `.gitignore` 覆盖);
5. 版本已 bump 且与将要打的 tag 一致。

## 三、发布步骤

```bash
# 1) 改版本(唯一版本源)
$EDITOR Cargo.toml            # [workspace.package] version = "0.7.0"
# 2) 提交(需维护者授权: 本项目要求用户明确同意才提交)
git commit -am "release: 0.7.0"
git push origin main
# 3) 打 tag 并推送(触发 release.yml: plan → 各平台构建 → 汇总校验和/安装器 → 发布 Release)
git tag v0.7.0
git push origin v0.7.0
# 4) 在 GitHub Release 页补写说明(要点: 平台矩阵、破坏性变更、已知问题)
```

- 预发布演练可用 `vX.Y.Z-rc.N`(CI 会标为 prerelease), 确认资产齐全后再发正式版。
- **变更记录策略(2026-09-15 决定)**: 只用 GitHub Release notes, 不建 `CHANGELOG.md`(宪法/决策 D7)。
- 发布后同步镜像: `git push gitee main --tags`(gitee 为只读镜像, 发布与协作只在 GitHub)。

## 四、多平台构建(dist, 2026-09-15 接入)

- 工具: `dist`(原 cargo-dist) **0.32.0**; 配置在 `dist-workspace.toml`(根), CI 工作流 `.github/workflows/release.yml` 由 `dist generate` 生成 —— **两个文件都不要手改**(改配置后重跑 `dist generate`)。
- 实际配置: `targets` = 5 个(见 §五 表); `installers = ["shell", "powershell"]`; `ci = "github"`; `hosting = "github"`。
- 产物(本地 `dist plan` 已核对): `ricow-<target>.tar.xz` / `x86_64-pc-windows-msvc.zip`(内含 `ricow`/`ricow.exe` + LICENSE + README)、
  每个产物一个 `.sha256` + 汇总 `sha256.sum`、`ricow-installer.sh`(curl | sh)、`ricow-installer.ps1`(irm | iex)、`source.tar.gz`。
- 维护者命令: `dist plan`(本地核对将构建什么, 不编译); `dist build`(本机只构建 host 平台); 版本升级流程见 §三。
- 首次发布: `v0.7.0`(2026-09-15, 项目进度约 70%)。

## 五、平台矩阵(以 CI 实测为准)

| 平台 | 构建 | 交易流程实机验证 | 是否发布产物 |
|---|---|---|---|
| Linux x86_64 (`x86_64-unknown-linux-gnu`) | ✅ 本地实测 | ✅ 已实测(回测 / demo 现货与合约) | ✅ 已发布 |
| Linux aarch64 (`aarch64-unknown-linux-gnu`) | CI 构建(GitHub ARM runner) | ❌ 未实机 | ✅ 已发布(Release 说明注明未实机验证) |
| Windows x86_64 (`x86_64-pc-windows-msvc`) | ✅ CI 实测(2026-09-15 `test (windows-latest)` 全绿: 编译 + 全量单元测试) | ❌ 未实机 | ✅ 已发布(同上注明) |
| macOS x86_64 / aarch64(`*-apple-darwin`) | CI 构建(GitHub macOS runner) | ❌ 未实机 | ✅ 已发布(同上注明) |

> 发布产物的平台 ≠ 宣称支持: 产物由 CI 交叉构建(github runner), **交易流程未实机验证的平台一律在 README 平台表与 Release 说明中标注"未实机"**。

> 规则: **未经验证的平台不得在 README / Release 说明中宣称支持**。状态变化时, 本表与 README「平台支持」表必须同步更新。

## 六、密钥与 CI 边界

- CI 只跑纯逻辑测试; 需要交易所密钥的 `#[ignore]` 用例**不进 CI**(宪法原则一「数据不出本机」), 由维护者本地带 demo key 执行。
- 不把任何交易所/LLM 密钥写入 GitHub Secrets。
