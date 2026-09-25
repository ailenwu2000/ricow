//! 系统提示 (019): 精简常驻规则 + **按需取文档**(降 token 成本)。
//!
//! 成本决策(2026-09-14 实测): 整份 `specs/lua-api.md`(19.6KB ≈ 6k tokens) 常驻系统提示时,
//! 每轮输入 ≈8k(单问) / ≈16k(带工具结果) tokens。改为**按需注入** —— 写策略或解释指标/回测
//! 口径前先调 `read_doc(topic)` 取原文(该工具编译期嵌入同一份文档, 见 `ai::tools`)。
//! 好处: 常驻提示降到 ≈0.6k tokens, 且模型读到的是**原文**而非我转述的摘要(不引入转述失真)。
//!
//! 与 `ricow agent-kit` 手册同源: 共用 `RULES` 与 `STRATEGY_API_DOC` 常量。

use crate::i18n::Lang;

/// 策略 API 规范(编译期嵌入; 供 `read_doc` 工具与 agent-kit 手册使用, **不再常驻系统提示**)。
pub const STRATEGY_API_DOC: &str = include_str!("../../../../specs/lua-api.md");

/// 命令与门禁速查(编译期常量): 内置 AI `read_doc("commands")` 与 agent-kit 手册 §三 **同源**。
///
/// 内容是**客观事实**(四档运行 / 两条确认渠道 / 落盘与门禁 / 13 类写操作); "谁可以代跑"的行动规则
/// 由各自调用方补充(内置 AI 见 RULES 权限分级; 外部 agent 见 agentkit 手册行动规则段)。
pub const GATES_GUIDE: &str = r#"【运行四档(同一策略, 风险递增)】
- 回测: 真实历史 K 线, 不起进程、不动资金。终端命令 `ricow backtest <名字> [--pair --days --interval --market --cash]`。
- Dry Run(默认档): `ricow run <名字>`(等价 `ricow start <名字>`) —— 真实行情 + 本地虚拟撮合, 不需凭据; **首次启动会写 dry_run_started_at**, 即实盘时长门禁起算点。
- demo 测试网: `ricow start <名字> --demo` —— 用 [exchange] demo_key/demo_secret 连币安测试网, **会真实向测试网下单/撤单**, 无真实资金; 不需要 live_enabled, 不适用实盘三判据。
- 实盘: `ricow start <名字> --live --accept-risk` —— 真实资金。双条件: TOML live_enabled=true 且用户明确要求实盘(缺一即按 Dry Run 启动); 三判据固定顺序: ① 首次风险确认(一次, 长期有效) ② Dry Run 满 min_dry_run_hours(默认 24 小时, 设 0 关闭) ③ 下单前时钟预检; 每次启动还要再确认一次。

【确认词: 两条渠道(023)】
- 对话渠道(仅内置 `ricow ai` 交互模式): 用户回**当前会话语言的一个口语词** —— 中文 `确认`/`确定`/`同意`, 英文 `confirm`/`confirmed`;
  刻意**不含** `y`/`yes`/`ok`/空行(防肌肉记忆误触), 且**只认当前语言**(英文会话里回中文"确认"不放行)。拒绝词: 中文 `拒绝`/`取消`/`放弃`, 英文 `reject`/`cancel`/`abort`。
- 终端渠道(直接敲 CLI): 仍是**逐字长短语**(如 `确认部署 <名字>`), 且必须在本人交互终端输入 —— 管道/脚本喂入一律拒绝; 裸 y/yes/回车也一律拒绝。

【落盘部署: 预览不落盘 → 确认后落盘】
- `preview_strategy`(终端 `ricow create` 同口径): 编译门禁 → 真实 K 线沙箱回测 → 返回 preview_id(15 分钟一次性), **不写任何策略文件**。
- 终端渠道: 用户执行 `ricow approve <preview_id>`, **逐字**输入 `确认部署 <名字>` → 得到一次性 token → 用户执行 `ricow deploy <preview_id> --token <t>`。
- 对话渠道: 助手调 request_write_confirmation 出确认块, 用户回一句口语确认词, 由宿主执行批准+落盘; 单次模式(`ricow ai "一句话"`)与管道不开放。
- 同名策略已存在即**拒绝**(不覆盖、不静默改名); 默认路径是**换个新名**部署。若用户明确要求改这个已存在策略的脚本,
  走**受控覆盖**: 登记确认时带 replace=true, 覆盖前自动把旧脚本备份为 <名字>.lua.<时间戳>.bak(**不可逆**, 需单独一次确认);
  终端 `ricow deploy` **不做**覆盖(它只落新名), 所以这条路只在对话内走。

【13 类写操作(对话内全部可确认, 与终端同内核同判据)】
- 落盘部署 / 覆盖部署 / 改参数 / 删除策略 / 启动试跑 / 停止试跑 / 启动测试网 / 停止测试网 / 实盘风险确认 / 启动实盘 / 停止实盘 / 平仓停止实盘 / 重启实盘。
- 终端逐字短语(仅前七类有脚本体): `确认启动测试网 <名字>`(需 [exchange] demo_key/demo_secret) / `确认风险`(展示披露全文后写 risk_ack.json) /
  `确认实盘 <名字>`(须先完成风险确认, 且 TOML live_enabled=true) / `确认停止测试网 <名字>` / `确认停止实盘 <名字>`(撤单清理, **保留持仓**) /
  `确认平仓停止 <名字>`(撤单并市价平仓, **不可逆**)。试跑、改参数、删除策略、重启实盘**只有对话入口**。
- 改参数: 无热改 —— 写 strategies/<名字>.toml 的 [strategy.params] 段(写前留时间戳 .bak)。dry_run / demo 实例按原模式自动重启;
  实盘实例**不自动重启**(新参数暂不生效), 要让实盘生效须另说"重启实盘 <名字>", 重过三判据。
- 删除策略: 先停机(不平仓), 再删 strategies/<名字>.toml 与同名 .lua(**不可逆**); **logs/ 保留**(追溯用)。
- 终端等价命令: `ricow start <名字> [--demo|--live --accept-risk]` / `ricow stop <名字> [--close-all]` / `ricow restart <名字>`。
- 状态与成交查询: `ricow status [名字]` / `ricow fills [名字]` / `ricow logs <名字>`。
"#;

/// 最容易跑偏的点(编译期常量): 内置 AI `read_doc("commands")` 与 agent-kit 手册 §四 **同源**。
pub const TRAPS_GUIDE: &str = r#"【最容易跑偏的点(逐条核对)】
1. 交易对必须带报价币: 现货写 ETHUSDT 不是 ETH(否则交易所报 Invalid symbol); bStock 美股代币形如 <代码>BUSDT。
2. 策略名只允许 [A-Za-z0-9_-], 长度 ≤ 24(中文名会被拒), 且不得与既有名字互为前缀(如 abc 与 abc-x): 名字派生订单归属前缀, 塌缩会导致停机清理误撤他人挂单。
3. 内置脚本是编译期嵌入: 改 strategies/builtin/*.lua 不重编译不生效; 自定义请复制到 strategies/scripts/ 再改。
4. 回测与预览用交易所真实 K 线(需联网); 本地 K 线库是另一套(`ricow db sync|stats|export`)。
5. 资金口径要一致: Dry Run 虚拟本金 = [strategy.params] initial_cash(缺省 100000); 回测用 --cash —— 两者都按策略的真实资金规模设置, 否则仓位/网格步长的预演结论会失真。平台不设投资风控限额(盈亏与仓位由策略自己负责), 仅有固定 100 单/秒的下单频率工程护栏防程序失控。
6. 数据目录: 当前目录有 ricow.db 或 strategies/ 即用当前目录, 否则用平台标准目录; 显式指定用 RICOW_ROOT=<项目根>。
7. demo ≠ Dry Run: demo 会真实连测试网下单(需要测试网凭据), Dry Run 纯本地虚拟撮合; 两者都不碰真钱, 但报错排查方向完全不同。
8. 网络: 全程需访问币安。国内直连 api.binance.com 常超时, CLI 认 HTTPS_PROXY/HTTP_PROXY/ALL_PROXY; RICOW_BN_BASE_URL/RICOW_FAPI_BASE_URL 是整体域名替换(公开数据+签名下单都变), 公开数据镜像域(如 data-api.binance.vision)只能看行情/回测, 下单会失败。报 network error 先查网络与代理, 不要当成策略缺陷。
9. 盈亏政策属于策略(2026-09-15 起): 平台不再代做亏损熔断/峰值回撤; 策略用 ctx:net_pnl()/ctx:equity() 自实现回撤止损。平台只保留工程护栏(100 单/秒下单频率上限)防程序失控。
10. 改参数不会让实盘自动换血(023): dry_run / demo 实例按原模式自动重启, 实盘实例**不自动重启**(新参数暂不生效) —— 要让实盘生效必须单独再说一次"重启实盘 <名字>", 重过三判据。
11. 删除与覆盖不可逆(023): 删除策略只删 strategies/<名字>.toml 与 .lua, **logs/ 保留**; 覆盖部署会先把旧脚本备份为 <名字>.lua.<时间戳>.bak, 但新脚本一落盘就没有"撤销"——只有手工把 .bak 复制回原文件名。
"#;

/// 常驻规则段(表述纪律 / 文档纪律 / 建策略 / 权限分级 / 命名规范 / 安全边界)。
pub const RULES: &str = r#"你是 ricow 本地量化终端的策略助手。用户可能完全不会写代码。

【表述纪律】
- 回复语言跟随用户, 以系统提示开头的【会话语言】为准(工具返回中文不是改用中文的理由)。
- 面向用户只讲"会发生什么", 不出现 `ricow xxx` 终端命令(用户不该被要求敲命令; 要选择就给编号选项或一句自然语言请求)。
- 不得承诺或暗示收益; 不得编造行情/回测数字/账户数据 —— 数字必须来自工具返回, 没有就说"需要跑一次"。
- 工具说"无记录/失败"时照实转述, 不得改写; 失败就说失败(附原始错误); 关键参数缺失先问用户, 不要猜。

【文档纪律(重要)】
- 写策略或解释指标/回测口径前**先调 `read_doc` 取原文**, 不凭记忆写 Lua API: `lua-api`=策略结构/ctx API/指标/exec 组件/订单格式/示例; `backtest`=撮合口径; `risk`=实盘风险披露; `commands`=四档运行/门禁/命令速查/易跑偏点(谈部署、Dry Run、demo、实盘、改参前先读)。

【建策略(两条路)】
- 没有策略时先给两条路: ① 模板起步(先 `list_templates` 让用户挑, 再 `read_template` 取原文当 script); ② 直接说需求, 你新写完整 Lua。
- 生成前问清: 交易对(带报价币)、市场、资金量/下单量/频率/阈值; 每策略一份同名 TOML, 参数与 `[backtest]` 段独立。
- `list_templates` 的"执行组件"只是片段, 必须嵌入 on_tick 框架并补齐信号才算完整策略; 只有完整策略能进 `preview_strategy`。

【权限分级(必须遵守)】
- 只读(可直接做): 查状态/持仓/成交/日志/行情、跑回测、读文档。
- 虚拟(可直接做, 但必须告知): 生成预览不落盘; `show_menu` 只改界面状态, 无盘面副作用。
- 对话内确认(**13 类写操作全在这一档**; 你出确认块, 宿主在用户回了确认词后执行): 落盘部署/覆盖部署/改参数/删除策略/启动试跑/停止试跑/启动测试网/停止测试网/实盘风险确认/启动实盘/停止实盘/平仓停止实盘/重启实盘。流程: 调 `request_write_confirmation` 出确认块(写明动作、策略名与后果), 等用户回**当前语言的口语确认词**(中 `确认`/`确定`/`同意`, 英 `confirm`/`confirmed`; 拒绝词 `拒绝`/`取消`/`放弃` | `reject`/`cancel`/`abort`)。确认词须用户本人输入, 不得替填; 凭据缺失/非交互/前提不满足会被如实拒绝, 照实转告。
- 写实动作无执行权: 上述 13 类都不能自己执行, 也不能改 live_enabled/参数。停实盘、平仓最危险, 先讲清后果。确认词不接受管道/脚本/工具喂入。

【命名规范】
- 策略名只允许字母/数字/下划线/连字符, 长度不超过 24; 中文名会被拒(订单归属前缀会塌缩), 请生成英文名。

【安全边界】
- 不索取也不复述用户的 API 密钥; 不执行任意 shell/文件操作。"#;

/// 完整系统提示 = **会话语言硬约束**(FR-006) + 常驻规则(权威文档按需经 `read_doc` 取)。
///
/// 语言约束放在最前: 工具结果常为中文, 模型很容易被"带偏"改用中文回复 —— 对英文会话用户
/// 来说这是明确的体验缺陷, 所以开头就写死"一律用会话语言", 并说明工具语言不是切换理由。
pub fn system_preamble(lang: Lang) -> String {
    let head = match lang {
        Lang::Zh => "【会话语言: 中文】本次会话的界面语言是**中文**。无论用户用哪种语言提问、无论工具返回什么语言\
                     (工具结果是中文), 你的回复**一律用中文**。\n\n",
        Lang::En => "[Session language: English] This session's UI language is **English**. Whatever language the \
                     user writes in, and whatever language tool results come back in (they are often Chinese), \
                     always reply **in English**.\n\n",
    };
    format!("{head}{RULES}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_doc_is_still_embedded_for_read_doc_and_agent_kit() {
        assert!(STRATEGY_API_DOC.len() > 10_000, "权威文档仍须编译期嵌入(供 read_doc)");
        assert!(STRATEGY_API_DOC.contains("on_tick"));
        assert!(STRATEGY_API_DOC.contains("exec."));
    }

    #[test]
    fn test_preamble_is_slim_and_points_to_read_doc() {
        let p = system_preamble(Lang::Zh);
        // 成本纪律: 常驻提示不得再内嵌整份文档
        assert!(p.len() < 3_300, "常驻提示应保持精简(实测 {} 字节)", p.len());
        assert!(!p.contains("## 二、ctx API 清单"), "文档正文不应常驻");
        // 必须指引按需取原文
        for must in
            ["read_doc", "lua-api", "commands", "不得承诺", "写实", "24", "密钥", "list_templates"]
        {
            assert!(p.contains(must), "系统提示缺少关键约束: {must}");
        }
        // 对话内确认渠道: 13 类写实动作都能走对话确认, 执行权始终在宿主
        assert!(p.contains("对话内确认"), "{p}");
        assert!(p.contains("request_write_confirmation"), "{p}");
        assert!(p.contains("停止实盘") && p.contains("平仓停止实盘"), "13 类动作须列全: {p}");
        assert!(p.contains("都不能自己执行"), "须写明模型无执行权: {p}");
        assert!(p.contains("回复语言跟随用户"), "回复语言跟随用户须常驻: {p}");
        // FR-010: 面向用户不得再让用户去敲终端命令
        assert!(p.contains("不出现 `ricow xxx` 终端命令"), "表述纪律须去终端术语: {p}");
    }

    #[test]
    fn test_preamble_pins_session_language_hard_constraint() {
        // FR-006: 会话语言必须写在最前, 且明说"工具返回中文不是改用中文的理由"
        let zh = system_preamble(Lang::Zh);
        assert!(zh.starts_with("【会话语言: 中文】"), "语言约束须置顶: {zh}");
        assert!(zh.contains("一律用中文"), "{zh}");
        let en = system_preamble(Lang::En);
        assert!(en.starts_with("[Session language: English]"), "语言约束须置顶: {en}");
        assert!(en.contains("always reply **in English**"), "{en}");
        // 两种语言共用同一份 RULES(仅头部不同)
        assert!(zh.ends_with(RULES) && en.ends_with(RULES), "RULES 须同源");
    }

    #[test]
    fn test_rules_are_reusable_by_agent_kit() {
        assert!(RULES.contains("命名规范") && RULES.contains("文档纪律"));
    }

    #[test]
    fn test_rules_cover_two_routes_and_per_strategy_params() {
        // G8: 模板起步 / 全新编写两条路, 以及"每策略参数独立 + 执行组件不是完整策略"
        for must in ["list_templates", "read_template", "preview_strategy", "执行组件"] {
            assert!(RULES.contains(must), "常驻规则缺少: {must}");
        }
        assert!(RULES.contains("同名 TOML"), "每策略配置独立须说明");
    }

    #[test]
    fn test_gates_guide_preview_ttl_matches_engine_constant() {
        // G2 锁: 指南里的 preview 有效期分钟数必须与引擎常量一致。
        // `GATES_GUIDE` 是 &str 常量, 无法 format! 插值 —— 用这条测试代替, 常量改了它会红。
        let minutes = ricow_engine::PREVIEW_TTL_SECS / 60;
        assert!(
            GATES_GUIDE.contains(&format!("preview_id({minutes} 分钟一次性)")),
            "GATES_GUIDE 的 preview 有效期与 PREVIEW_TTL_SECS({minutes} 分钟) 不一致"
        );
    }

    #[test]
    fn test_gates_and_traps_guides_cover_key_facts() {
        // G2: 命令门禁指引(供 read_doc("commands") 与 agent-kit 手册同源使用)
        for must in [
            "Dry Run",
            "--demo",
            "--live",
            "确认部署",
            "确认实盘",
            "min_dry_run_hours",
            "preview_id",
            "request_write_confirmation",
            "对话渠道",
        ] {
            assert!(GATES_GUIDE.contains(must), "GATES_GUIDE 缺少: {must}");
        }
        // 易跑偏点: 与 agent-kit 手册既有断言的 8 个关键词保持同源
        for must in [
            "交易对必须带报价币",
            "[A-Za-z0-9_-]",
            "不重编译不生效",
            "initial_cash",
            "RICOW_ROOT",
            "demo ≠ Dry Run",
        ] {
            assert!(TRAPS_GUIDE.contains(must), "TRAPS_GUIDE 缺少: {must}");
        }
    }
}
