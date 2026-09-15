//! `ricow agent-kit` — 给**用户已有的外部 agent** 生成操作手册 (019 T045/T046, FR-031–FR-033)。
//!
//! 三件事:
//! 1. 手册内容与内置 AI **同源**: 准则段用 `ai::prompt::RULES`, 策略 API 原文用
//!    `ai::prompt::STRATEGY_API_DOC`(编译期嵌入的同一常量) —— 代码里只有一份, 不会漂移;
//! 2. 命令速查**由 clap 命令树在运行时生成**(不手抄), 因此不会与 `ricow --help` 不一致;
//! 3. 安装 4 个文件到用户指定目录: `AGENTS.md` / `SKILL.md` / `CLAUDE.md`(一行导入) / `lua-api.md`。
//!
//! 安全: **不覆盖**已存在的文件 —— 任一目标文件已存在且内容不同即整体拒绝(零写入),
//! 与 `ricow deploy` 同名拒绝同一思路(不静默破坏用户已有内容)。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::{Args, CommandFactory};
use ricow_core::{CoreError, CoreResult};

use crate::ai::prompt;

#[derive(Args)]
pub struct AgentKitArgs {
    /// 安装到指定目录(省略值 = 当前目录); 不带此参数时只把手册打印到 stdout
    #[arg(long, num_args = 0..=1, default_missing_value = ".")]
    pub install: Option<String>,
}

/// 生成的文件清单(名字 → 内容)。
fn artifacts() -> Vec<(&'static str, String)> {
    vec![
        ("AGENTS.md", manual()),
        ("SKILL.md", skill_md()),
        ("CLAUDE.md", "@AGENTS.md\n".to_string()),
        ("lua-api.md", prompt::STRATEGY_API_DOC.to_string()),
    ]
}

/// 命令速查(由 clap 命令树生成; `help` 子命令跳过)。
fn quickref() -> String {
    let cmd = crate::Cli::command();
    let mut out = String::new();
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        if name == "help" {
            continue;
        }
        let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
        let _ = writeln!(out, "- `ricow {name} …` — {about}");
    }
    out
}

/// `AGENTS.md` 正文(也是 `SKILL.md` 的正文)。
pub fn manual() -> String {
    format!(
        "# ricow 操作手册\n\
\n\
> 本文件由 `ricow agent-kit --install` 生成(019)。它与你机器上内置 AI 助手 `ricow ai` 的系统提示\n\
> **同源** —— 下面 §五 的准则与 `lua-api.md` 都取自同一份编译期常量, 不存在两套口径。\n\
> 你是外部 agent: 请用**本机 `ricow` 命令行**驱动它, 写 Lua 策略前先读同目录的 `lua-api.md`。\n\
\n\
## 一、ricow 是什么\n\
本地量化终端(headless 引擎 + CLI; 数据、密钥、成交都在本机, 不托管): 回测 / Dry Run(真实行情 + 虚拟成交) / 币安实盘。\n\
你能做的: 选币、回测、生成策略(写 Lua)、查看状态/成交/盈亏/日志、起停 Dry Run。\n\
你**不能**做的: 落盘部署、实盘启停、平仓、改参数与风控 —— 由用户本人在自己终端确认执行(见 §三)。\n\
\n\
## 二、命令速查(由 CLI 自身生成, 不会过时)\n\
{quickref}\n\
具体参数一律以 `ricow <命令> --help` 为准, 不要凭猜测拼参数。\n\
\n\
## 三、写实动作与门禁(**不可绕过**)\n\
1. `ricow create` **不落盘**: 编译门禁 → 真实 K 线沙箱回测 → 返回 `preview_id` + 报告。落盘要三步:\n\
   用户执行 `ricow approve <preview_id>`(**逐字**输入 `确认部署 <名字>`; 裸 `y`/回车不接受; **且必须在他自己的交互终端输入** ——
   本命令拒绝管道/脚本喂入的短语, 你用工具调用喂进去只会得到「确认必须在交互终端输入」的报错) → 得到一次性 token\n\
   → 用户执行 `ricow deploy <preview_id> --token <t>`。**你不要代跑 approve/deploy**, 只把这两条命令告诉用户。\n\
2. 同名策略已存在即**拒绝**(不覆盖、不静默改名); 要替换需先由用户移除旧文件或换个名字。\n\
3. 实盘三判据(顺序固定): ① 首次实盘风险确认(`ricow start --accept-risk <名字>`, 一次) → ② Dry Run 时长门禁\n\
   (`[strategy.params] min_dry_run_hours`, 默认 24 小时, 可设 0 关闭) → ③ 下单前时钟预检。\n\
   每次实盘启动还需用户**逐字**输入 `确认实盘 <名字>`。Dry Run 与回测不受这三条影响。\n\
4. `--live` 需策略 TOML 里 `live_enabled = true`(双条件, 缺一即按 Dry Run 启动); `--demo` 走币安测试网, 不适用实盘三判据。
   实盘启动的 `确认实盘 <名字>` 同样**只能在交互终端**输入(管道无效)。\n\
5. Dry Run 首次启动会写 `dry_run_started_at` —— **这是计时开始**(实盘时长门禁的依据), 不是\"无副作用\"。\n\
\n\
## 四、最容易跑偏的点(逐条核对)\n\
1. **交易对必须带报价币**: 现货写 `ETHUSDT`, 不是 `ETH`(否则交易所返回 `Invalid symbol`);\n\
   bStock 美股代币形如 `<代码>BUSDT`。\n\
2. **策略名规范**: 只允许 `[A-Za-z0-9_-]`, 长度 ≤ 24(中文名会被系统拒绝), 且不得与既有策略名**互为前缀**(如 `abc` 与 `abc-x`)。\n\
   原因: 名字会派生订单归属前缀, 塌缩或互为前缀会导致停机清理误撤他人挂单。建议 `eth-grid-300` 这类英文短名。\n\
3. **内置脚本是编译期嵌入**: 改 `strategies/builtin/*.lua` 文件**不重编译不生效**; 自定义请复制到 `strategies/scripts/` 再改。\n\
4. **回测与预览用的是交易所真实 K 线**(需联网); 本地 K 线库是另一套(`ricow db sync|stats|export`)。\n\
5. **资金口径**: Dry Run 虚拟本金 = `[strategy.params] initial_cash`(缺省 100000); `ricow backtest` 用 `--cash`。\n\
   它与 `[risk]` 限额必须**同一口径** —— 小资金配大限额会被风控大量拒单。\n\
6. **数据目录**: 默认取当前工作目录(在项目根跑 `ricow`); 在别处运行请设 `RICOW_ROOT=<项目根>`。\n\
7. 不确定就问用户: 缺交易对/天数/资金量等关键参数时先确认, 不要自己猜; 工具或命令报错时**照实转述原文**, 不要润色。
8. **网络**: ricow 全程需要访问币安(公开行情 + 签名端点)。国内需代理 —— CLI 认 `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`。
   报 `network error` 时先确认代理(或 `RICOW_BN_BASE_URL` 指向可用域名), 不要把失败当「策略有问题」。
   `RICOW_BN_BASE_URL`/`RICOW_FAPI_BASE_URL` 是**整体域名替换**(公开数据 + 签名下单都变); 公开数据镜像域(如 `data-api.binance.vision`)只能回测看行情, 下单会失败。
9. **盈亏政策属于策略**(2026-09-15 起): 平台**不再**代做亏损熔断/峰值回撤; 策略用 `ctx:net_pnl()` / `ctx:equity()` 自己实现回撤与止损
   (内置 `shannon_grid` 的 `dd_stop_pct` 是参考写法)。平台只保留工程护栏(下单频率上限)与你显式配置的静态限额。\n\
\n\
## 五、准则(与内置 AI 系统提示同源, 逐字)\n\
{rules}\n\
\n\
## 六、本目录文件\n\
- `AGENTS.md` 本手册(Codex / Cursor / Copilot / Gemini CLI / Aider / Zed / Windsurf 等原生读取)\n\
- `CLAUDE.md` 一行导入 `@AGENTS.md`(Claude Code 不读 AGENTS.md)\n\
- `SKILL.md` Agent Skills 标准(Claude Code / Codex 支持, 按需加载)\n\
- `lua-api.md` 权威策略 API 原文(与 `ricow ai` 的 `read_doc(\"lua-api\")` 同一常量) —— 写策略前先读它\n",
        quickref = quickref(),
        rules = prompt::RULES
    )
}

/// `SKILL.md` = YAML frontmatter(Agent Skills 标准) + 与 `AGENTS.md` 相同的正文。
pub fn skill_md() -> String {
    format!(
        "---\n\
name: ricow\n\
description: 用本机 ricow 本地量化终端(回测 / Dry Run / 币安实盘)做事: 选币、写 Lua 策略、沙箱回测、\
查看状态与成交、起停 Dry Run; 落盘部署与实盘只能由用户本人在自己终端执行。\
Use when 用户要用 ricow 回测策略、生成/修改 Lua 策略、看成交或盈亏、启停策略。\n\
---\n\
\n\
{}",
        manual()
    )
}

/// 安装结果。
pub enum InstallOutcome {
    /// 已写入这些路径(可能有部分文件此前已是最新, 不计入)。
    Installed(Vec<PathBuf>),
    /// 四个文件都已存在且内容一致 → 未改动。
    UpToDate,
    /// 这些文件已存在且内容不同 → 整体拒绝, **零写入**。
    Refused(Vec<PathBuf>),
}

/// 安装到目录(先全量检查再写: 有冲突就一个文件都不写)。
pub fn install_into(dir: &Path) -> CoreResult<InstallOutcome> {
    let mut conflicts: Vec<PathBuf> = Vec::new();
    let mut to_write: Vec<(PathBuf, String)> = Vec::new();
    for (name, content) in artifacts() {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(existing) if existing == content => {}
            Ok(_) => conflicts.push(path),
            Err(_) if path.exists() => conflicts.push(path),
            Err(_) => to_write.push((path, content)),
        }
    }
    if !conflicts.is_empty() {
        return Ok(InstallOutcome::Refused(conflicts));
    }
    if to_write.is_empty() {
        return Ok(InstallOutcome::UpToDate);
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| CoreError::Parse(format!("创建目录失败 {}: {e}", dir.display())))?;
    for (path, content) in &to_write {
        std::fs::write(path, content)
            .map_err(|e| CoreError::Parse(format!("写入失败 {}: {e}", path.display())))?;
    }
    Ok(InstallOutcome::Installed(to_write.into_iter().map(|(p, _)| p).collect()))
}

pub fn run(args: AgentKitArgs) -> CoreResult<()> {
    let Some(dir) = args.install else {
        print!("{}", manual());
        return Ok(());
    };
    let dir = PathBuf::from(dir);
    match install_into(&dir)? {
        InstallOutcome::Installed(paths) => {
            println!("已安装 agent 手册到 {}:", dir.display());
            for p in paths {
                println!("  {}", p.display());
            }
            println!("下一步: 让你用的 agent 在**这个目录**里工作(AGENTS.md / CLAUDE.md / SKILL.md 会被自动读取),");
            println!("        写策略前它会先读 lua-api.md; 落盘与实盘仍需你本人在终端执行 approve/deploy。");
            Ok(())
        }
        InstallOutcome::UpToDate => {
            println!("已是最新: {} 下的 4 个文件内容与当前版本一致, 未改动。", dir.display());
            Ok(())
        }
        InstallOutcome::Refused(conflicts) => {
            let list = conflicts
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n");
            Err(CoreError::InvalidArgument(format!(
                "以下文件已存在且内容不同, 拒绝覆盖(**未写任何文件**):\n{list}\n\
                 请先备份或移除它们(或换一个目录: `ricow agent-kit --install <目录>`), 再重新安装。"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手册必须覆盖\"AI 最容易跑偏的点\"(FR-033 / T046)。
    #[test]
    fn test_manual_covers_traps() {
        let m = manual();
        for must in [
            "交易对必须带报价币", // ETHUSDT 而非 ETH
            "不落盘",             // create 不落盘
            "逐字",               // approve / 实盘逐字确认
            "min_dry_run_hours",  // 实盘时长门禁
            "不重编译不生效",     // 内置脚本编译期嵌入
            "[A-Za-z0-9_-]",      // 命名规范
            "initial_cash",       // 资金口径
            "RICOW_ROOT",         // 数据目录
        ] {
            assert!(m.contains(must), "手册缺少易跑偏点: {must}");
        }
    }

    /// 同源(FR-032): 准则段逐字来自内置 AI 的系统提示常量。
    #[test]
    fn test_manual_reuses_prompt_constants() {
        let m = manual();
        assert!(m.contains(prompt::RULES), "手册必须逐字包含 prompt::RULES");
        assert!(prompt::RULES.contains("权限分级") && prompt::RULES.contains("命名规范"));
    }

    /// 命令速查由 clap 生成 → 与 CLI 不漂移(新增/删除命令会在此暴露)。
    #[test]
    fn test_quickref_lists_every_cli_command() {
        let q = quickref();
        for sub in crate::Cli::command().get_subcommands() {
            let name = sub.get_name();
            if name == "help" {
                continue;
            }
            assert!(q.contains(&format!("`ricow {name} …`")), "速查缺少命令: {name}");
        }
        assert!(!q.contains("`ricow help …`"), "help 不应进速查");
        assert!(q.contains("`ricow ai …`") && q.contains("`ricow create …`"));
    }

    #[test]
    fn test_skill_md_has_frontmatter_and_body() {
        let s = skill_md();
        assert!(s.starts_with("---\nname: ricow\n"), "{s:.80}");
        assert!(s.contains("description: 用本机 ricow"), "缺 description");
        assert!(s.contains("# ricow 操作手册"), "正文应为同一份手册");
        assert!(s.contains(prompt::RULES), "SKILL.md 同样同源");
    }

    #[test]
    fn test_artifacts_include_doc_and_claude_import() {
        let files: Vec<String> = artifacts().into_iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(files, vec!["AGENTS.md", "SKILL.md", "CLAUDE.md", "lua-api.md"]);
        let claude = artifacts().into_iter().find(|(n, _)| *n == "CLAUDE.md").unwrap().1;
        assert_eq!(claude, "@AGENTS.md\n");
        let api = artifacts().into_iter().find(|(n, _)| *n == "lua-api.md").unwrap().1;
        assert_eq!(api, prompt::STRATEGY_API_DOC, "策略 API 必须与 read_doc 同一常量");
    }

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ricow-agentkit-{tag}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn test_install_writes_four_files_then_reports_up_to_date() {
        let dir = tmp("install");
        match install_into(&dir).unwrap() {
            InstallOutcome::Installed(paths) => assert_eq!(paths.len(), 4),
            _ => panic!("首次安装应写入 4 个文件"),
        }
        for name in ["AGENTS.md", "SKILL.md", "CLAUDE.md", "lua-api.md"] {
            assert!(dir.join(name).exists(), "缺文件: {name}");
        }
        // 二次安装: 内容一致 → 不写, 报已是最新
        assert!(matches!(install_into(&dir).unwrap(), InstallOutcome::UpToDate));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_install_refuses_overwrite_and_writes_nothing() {
        let dir = tmp("refuse");
        std::fs::create_dir_all(&dir).unwrap();
        // 用户已有的 AGENTS.md(内容不同)
        std::fs::write(dir.join("AGENTS.md"), "# 我自己的 agent 说明\n").unwrap();
        match install_into(&dir).unwrap() {
            InstallOutcome::Refused(paths) => assert_eq!(paths.len(), 1),
            _ => panic!("已存在且内容不同 → 必须拒绝"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(),
            "# 我自己的 agent 说明\n",
            "拒绝时不得改写用户文件"
        );
        for name in ["SKILL.md", "CLAUDE.md", "lua-api.md"] {
            assert!(!dir.join(name).exists(), "拒绝时不得写任何文件: {name}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
