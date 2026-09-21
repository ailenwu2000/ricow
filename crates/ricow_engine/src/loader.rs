//! 策略装载: 从配置实例化策略。
//!
//! 策略层统一 Lua: 内置策略与用户策略均为 Lua 脚本, 由 CLI 层
//! (resolve_builtin_script / script_path) 注入 `script` 参数后,
//! 这里只做 LuaStrategy 实例化。
//!
//! 历史注记 (2026-09-11 下线): 首个 Lua 组合策略 bs_momentum 已删除 (真实成交轨期望 ≈0,
//! 每 bar spot −0.0198% / futures +0.0367%, 不合格)。组合回测入口
//! 机制保留在 ricow / ricow_strategy (通用能力, 暂无内置消费者)。

use ricow_core::{CoreError, CoreResult};
use ricow_strategy::{HostServices, LuaStrategy, Strategy, StrategyConfig};

/// 根据配置实例化策略 (全部为 Lua; 内置名经 CLI 层 Lua 化后以 lua 类型到达)。
pub fn load_strategy(config: &StrategyConfig) -> CoreResult<Box<dyn Strategy>> {
    load_strategy_inner(config, None)
}

/// 同 [`load_strategy`], 但**在脚本顶层执行之前**注入宿主服务 (028 T016)。
///
/// 为什么必须在顶层之前: 策略可以在脚本顶层写 `data:series{...}` 声明数据面 —— 那一刻就要取数。
/// `load_strategy`(不注入宿主) 只适合不使用 `data:*`/`http:*` 的脚本。
pub fn load_strategy_with_host(
    config: &StrategyConfig,
    host: std::sync::Arc<dyn HostServices>,
) -> CoreResult<Box<dyn Strategy>> {
    load_strategy_inner(config, Some(host))
}

fn load_strategy_inner(
    config: &StrategyConfig,
    host: Option<std::sync::Arc<dyn HostServices>>,
) -> CoreResult<Box<dyn Strategy>> {
    match config.strategy_type.as_str() {
        "lua" => {
            let code = config.get_str("script").ok_or_else(|| {
                CoreError::InvalidArgument("lua 策略需要 'script' 参数 (lua 代码字符串)".into())
            })?;
            ricow_strategy::lua::validate_script_source(code)
                .map_err(CoreError::InvalidArgument)?;
            let strategy = match host {
                Some(h) => LuaStrategy::from_source_with_host(code, config.clone(), h),
                None => LuaStrategy::from_source(code, config.clone()),
            }
            .map_err(CoreError::InvalidArgument)?;
            Ok(Box::new(strategy))
        }
        other => Err(CoreError::InvalidArgument(format!("unsupported strategy type: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(t: &str) -> StrategyConfig {
        StrategyConfig {
            name: "t".into(),
            strategy_type: t.into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        }
    }

    /// 取错误文本 (Ok 分支 panic; dyn Strategy 无 Debug, 不能用 expect_err)。
    fn err_text(r: CoreResult<Box<dyn Strategy>>) -> String {
        match r {
            Err(e) => e.to_string(),
            Ok(_) => panic!("期望报错, 实际装配成功"),
        }
    }

    #[test]
    fn test_script_as_filename_gets_actionable_error() {
        // 实测坑: script = "x.lua" 会被当代码编译 → 报难懂的 syntax error; 现在给可执行指引
        let mut c = config("lua");
        c.params.insert("script".into(), ricow_strategy::ConfigValue::String("x.lua".into()));
        let err = err_text(load_strategy(&c));
        assert!(err.contains("内联 Lua 代码字符串"), "{err}");
        assert!(err.contains("script_path"), "{err}");
    }

    #[test]
    fn test_unsupported_type_still_rejected() {
        let err = err_text(load_strategy(&config("nonsense")));
        assert!(err.contains("unsupported strategy type"));
    }
}
