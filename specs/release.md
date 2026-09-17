# 发布规范(release)

> 本文是**发布流程与产物平台矩阵的唯一权威**。宪法「分支与发布」章为原则, 本文为可执行清单。
> 状态: 2026-09-15 建立; 同日接入多平台构建工具 dist(cargo-dist 0.32.0), 见 §四; 2026-09-17 补齐四类安装器与安装器实机状态口径。

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
- **gitee 镜像已停止维护**(2026-09-15 决定): 发布、协作、issue/PR 一律以 GitHub 为准;旧 locus 仓库留在 gitee 仅作历史存档, 不再推送。

## 四、多平台构建(dist, 2026-09-15 接入)

- 工具: `dist`(原 cargo-dist) **0.32.0**; 配置在 `dist-workspace.toml`(根), CI 工作流 `.github/workflows/release.yml` 由 `dist generate` 生成 —— **两个文件都不要手改**(改配置后重跑 `dist generate`)。
- 实际配置: `targets` = 5 个(见 §五 表); `installers = ["shell", "powershell", "homebrew", "msi"]`; `ci = "github"`; `hosting = "github"`;
  **`install-updater = false`**(FR-038/SC-011: 产物与安装器不得含自动更新组件);
  `include = ["packaging/启动-ricow-AI助手.cmd", "packaging/启动-ricow-AI助手.sh", "packaging/启动-ricow-AI助手.command"]`(FR-039 / T035: 三平台启动入口随包分发)。
- 启动入口(2026-09-17 补齐三平台, 同名不同后缀): `.cmd` = Windows 双击(`chcp 65001` + `ricow ai` + `pause`);
  `.sh` = Linux/macOS 终端(`./启动-ricow-AI助手.sh`, 结束时若 stdin 是 tty 则等回车以免双击开的终端窗口秒关);
  `.command` = macOS 双击(Finder 直接执行 `.command`, 该文件只是 `exec` 同名 `.sh` 的薄壳, 逻辑只有一份)。Unix 侧**不动 locale**(UTF-8 是默认, 中文由 ricow 自己输出)。
- 二进制查找链(2026-09-17 修订, `.cmd` 与 `.sh` 同一口径): 脚本同目录(`./ricow` / `ricow.exe`, 即压缩包布局) → 源码检出 `../target`
  —— release 与 debug **取最后写入时间较新的那个**: 两份产物各自会过期, 固定优先 release 会让一键启动跑在两日前的旧二进制上并按旧规则报错(实测: 旧 release 报 `[market]` 未知段) ——
  → `PATH`。命中 `../target` 时先 `cd` 到仓库根: `commands::resolve_root` 以当前目录是否含 `ricow.db`/`strategies/` 判定数据目录, 从 `packaging/` 直接跑会静默用/新建另一份空数据目录。
  三者都找不到则打印 `cargo build -p ricow` 提示并以退出码 1 结束(`.cmd` 侧另加 `pause`)。
- 产物(本地 `dist plan` 已核对): `ricow-<target>.tar.xz` / `x86_64-pc-windows-msvc.zip`(内含 `ricow`/`ricow.exe` + LICENSE + README.md + `启动-ricow-AI助手.cmd`/`.sh`/`.command`)、
  每个产物一个 `.sha256` + 汇总 `sha256.sum`、`source.tar.gz`,
  安装器四类: `ricow-installer.sh`(curl | sh)、`ricow-installer.ps1`(irm | iex)、`ricow.rb`(Homebrew 公式, 需 `brew install ./ricow.rb`, **无 tap**)、
  `ricow-x86_64-pc-windows-msvc.msi`(msi, 装到 `%ProgramFiles%\ricow\bin` 并追加 PATH; 只含二进制, 双击入口在 `.zip`)。
- msi 安装器由 cargo-dist 生成, 底层调用 **`cargo wix`**, 因此 manifest 必须有 `authors`(写成 WiX 的 Manufacturer)与
  `description`; GUID 由 `dist generate` 写入 `crates/ricow/Cargo.toml` 的 `[package.metadata.wix]`(`upgrade-guid` / `path-guid`), WiX 定义落 `crates/ricow/wix/main.wxs`。
  Homebrew 安装器要求 manifest 有 `homepage`, 否则 `dist plan` 打 WARN。
- FR-058(安装/解压后被系统拦截时如实提示并给绕行): cargo-dist 自带安装器模板**不含** quarantine/MOTW 检测(已核对 0.32.0 模板源码),
  故落在自有的启动入口内 —— `.cmd` 用 `dir /r | findstr Zone.Identifier` 检测 NTFS 备用数据流, 命中则打印 SmartScreen 绕行提示;
  `.sh` 用 `xattr` 查 `com.apple.quarantine`, 命中则打印 `xattr -dr com.apple.quarantine` 绕行提示(Linux 的 `.tar.xz` 无等价标记, 故不适用)。
- 维护者命令: `dist plan`(本地核对将构建什么, 不编译); `dist build`(本机只构建 host 平台); `dist generate`(配置改后重生成 CI 工作流与 WiX 定义);
  `dist generate --check`(校验已生成文件是否最新); 版本升级流程见 §三。
  另: `cargo ai` 是**仓库本地** alias(`.cargo/config.toml`, 等价 `cargo run -p ricow -- ai`)—— `ai` 是 ricow 子命令而非 cargo 子命令, 该 alias 只方便检出内开发, 不进发布产物。
- 首次发布: `v0.7.0`(2026-09-15, 项目进度约 70%)。

## 五、平台矩阵(以 CI 实测为准)

| 平台 | 构建 | 交易流程实机验证 | 是否发布产物 |
|---|---|---|---|
| Linux x86_64 (`x86_64-unknown-linux-gnu`) | ✅ 本地实测 | ✅ 已实测(回测 / demo 现货与合约) | ✅ 已发布 |
| Linux aarch64 (`aarch64-unknown-linux-gnu`) | CI 构建(GitHub ARM runner) | ❌ 未实机 | ✅ 已发布(Release 说明注明未实机验证) |
| Windows x86_64 (`x86_64-pc-windows-msvc`) | ✅ CI 实测(2026-09-15 `test (windows-latest)` 全绿: 编译 + 全量单元测试) | ❌ 未实机 | ✅ 已发布(同上注明) |
| macOS x86_64 / aarch64(`*-apple-darwin`) | CI 构建(GitHub macOS runner) | ❌ 未实机 | ✅ 已发布(同上注明) |

> 发布产物的平台 ≠ 宣称支持: 产物由 CI 交叉构建(github runner), **交易流程未实机验证的平台一律在 README 平台表与 Release 说明中标注"未实机"**。

> 安装器实机状态(2026-09-17): 四类安装器由**同一份配置**生成, 但只有 **shell 安装器在 Linux 实机跑过**;
> PowerShell / Homebrew / msi 三类**尚未在实机安装验证**(待 T057 干净机冒烟)。README 的平台表与「安装」章节按同一口径说明, 二者必须一致。

> 规则: **未经验证的平台不得在 README / Release 说明中宣称支持**。状态变化时, 本表与 README「平台支持」表必须同步更新。

## 六、密钥与 CI 边界

- CI 只跑纯逻辑测试; 需要交易所密钥的 `#[ignore]` 用例**不进 CI**(宪法原则一「数据不出本机」), 由维护者本地带 demo key 执行。
- 不把任何交易所/LLM 密钥写入 GitHub Secrets。
