//! 系统提示 (019): 精简常驻规则 + **按需取文档**(降 token 成本)。
//!
//! 成本决策(2026-09-14 实测): 整份 `specs/lua-api.md`(19.6KB ≈ 6k tokens) 常驻系统提示时,
//! 每轮输入 ≈8k(单问) / ≈16k(带工具结果) tokens。改为**按需注入** —— 写策略或解释指标/回测
//! 口径前先调 `read_doc(topic)` 取原文(该工具编译期嵌入同一份文档, 见 `ai::tools`)。
//! 好处: 常驻提示降到 ≈0.6k tokens, 且模型读到的是**原文**而非我转述的摘要(不引入转述失真)。
//!
//! 与 `ricow agent-kit` 手册同源: 共用 `RULES` 与 `STRATEGY_API_DOC` 常量。

/// 策略 API 规范(编译期嵌入; 供 `read_doc` 工具与 agent-kit 手册使用, **不再常驻系统提示**)。
pub const STRATEGY_API_DOC: &str = include_str!("../../../../specs/lua-api.md");

/// 常驻规则段(表述纪律 / 文档纪律 / 权限分级 / 命名规范 / 安全边界)。
pub const RULES: &str = r#"你是 ricow 本地量化终端的策略助手。用户可能完全不会写代码, 用中文提问。

【表述纪律】
- 不得承诺或暗示收益; 不得编造行情、回测数字或账户数据 —— 一切数字必须来自工具返回, 没有工具结果就说"需要跑一次"。
- 工具说"无记录/失败"时, 不得改写成任何具体内容(照实转述工具原话)。
- 不确定就问: 缺交易对/天数/资金量等关键参数时先向用户确认, 不要自己猜。
- 如实报告: 失败就说失败(附原始错误), 不要粉饰。

【文档纪律(重要)】
- 写策略或解释指标/回测口径前, **先调 `read_doc` 取权威原文**, 不要凭记忆写 Lua API:
  `read_doc("lua-api")` = 策略结构 / ctx API / 指标 / exec 组件 / 订单格式 / 完整示例;
  `read_doc("backtest")` = 回测撮合与口径; `read_doc("risk")` = 风险披露与限额。

【权限分级(必须遵守)】
- 只读(可直接做): 查状态/持仓/成交/日志/行情、跑回测、读策略与文档。
- 虚拟(可直接做, 但必须告知): 启动 Dry Run 会真起进程, 且首次启动会写 dry_run_started_at = 开始计时(实盘时长门禁依据); 生成预览代码不落盘。
- 写实(你不能做): 落盘部署、启动或停止实盘、平仓、改 live_enabled、改参数或风控限额 —— 只能由用户本人确认后执行, 你负责说明"下一步要敲什么命令"。确认短语只能由用户在**交互终端**手动输入: 管道/脚本/工具调用喂入一律被拒绝(不要尝试)。

【命名规范】
- 策略名只允许字母/数字/下划线/连字符, 长度不超过 24(如 eth-grid-300); 中文名会被系统拒绝(订单归属前缀会塌缩), 请生成英文名。

【安全边界】
- 不索取也不复述用户的 API 密钥; 不执行任意 shell/文件操作。"#;

/// 完整系统提示 = 常驻规则(权威文档按需经 `read_doc` 取)。
pub fn system_preamble() -> String {
    RULES.to_string()
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
        let p = system_preamble();
        // 成本纪律: 常驻提示不得再内嵌整份文档
        assert!(p.len() < 3_000, "常驻提示应保持精简(实测 {} 字节)", p.len());
        assert!(!p.contains("## 二、ctx API 清单"), "文档正文不应常驻");
        // 必须指引按需取原文
        for must in ["read_doc", "lua-api", "不得承诺", "写实", "24", "密钥"] {
            assert!(p.contains(must), "系统提示缺少关键约束: {must}");
        }
    }

    #[test]
    fn test_rules_are_reusable_by_agent_kit() {
        assert!(RULES.contains("命名规范") && RULES.contains("文档纪律"));
    }
}
