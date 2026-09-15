//! Lua 沙箱引擎工厂。
//!
//! 为策略脚本提供受限的 Lua 5.4 运行时 (mlua 0.11 嵌入式, vendored 编进二进制)。
//! 安全边界:
//! - 只加载安全标准库 (base/table/string/math/utf8), 不加载 os/io/debug/package/coroutine;
//! - `require` 不可用 (无 package 库); `load`/`loadstring`/`loadfile`/`dofile` 显式移除;
//! - `set_hook` 指令预算 (默认 1_000_000 条/tick), 死循环被拦截;
//! - `print` 重定向到 tracing 日志, 不污染 stdout。

use mlua::{Error, HookTriggers, Lua, MultiValue, StdLib};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 单 tick 指令预算。超过即中断 (防止死循环/过度计算)。
const INSTRUCTION_BUDGET: u64 = 1_000_000;
/// hook 触发粒度: 每 N 条指令检查一次 (预算按 N 的倍数消耗)。
const HOOK_GRANULARITY: u32 = 1_000;
/// Lua 堆内存上限 (防 string.rep 等单次 C 调用绕过指令预算造成 OOM)。
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// 指令预算句柄。每个回调周期开始前调用 [`SandboxBudget::reset`],
/// 使预算语义为"单 tick 1M 条指令"而非跨回调累积。
#[derive(Clone)]
pub struct SandboxBudget(Arc<AtomicU64>);

impl SandboxBudget {
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(budget_units())))
    }

    /// 回调周期开始前调用: 重置本 tick 的指令预算。
    pub fn reset(&self) {
        self.0.store(budget_units(), Ordering::Relaxed);
    }
}

fn budget_units() -> u64 {
    INSTRUCTION_BUDGET / u64::from(HOOK_GRANULARITY)
}

/// 创建配置好沙箱限制的 Lua 引擎。
///
/// 每个策略实例一个引擎 (跨回调复用, 保持模块级状态)。
/// 返回 (引擎, 预算句柄) — 预算必须每回调周期 reset, 见 [`SandboxBudget`]。
pub fn create_lua_sandbox() -> (Lua, SandboxBudget) {
    // base 库由 mlua 恒加载 (含 pairs/type/tonumber 等策略常用函数);
    // 只显式加载其余安全库, 不加载 os/io/debug/package/coroutine。
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        mlua::LuaOptions::default(),
    )
    .expect("create sandbox lua");

    // 堆内存上限: 单次 C 调用 (如 string.rep) 可绕过指令预算一次性分配巨量内存。
    lua.set_memory_limit(MEMORY_LIMIT).expect("set memory limit");

    let globals = lua.globals();

    // 移除 base 库中的动态加载入口与错误捕获入口:
    // - load/loadstring/loadfile/dofile: 动态加载;
    // - pcall/xpcall: 会捕获 hook 抛出的指令预算错误, 使死循环被 pcall 吞掉后
    //   CPU 无限燃烧 (预算逃逸向量, 实测确认)。策略代码不需要 pcall。
    // coroutine 库未加载, 脚本内无其他错误捕获途径 → hook 预算错误必达 Rust 侧。
    for name in ["load", "loadstring", "loadfile", "dofile", "pcall", "xpcall"] {
        let _ = globals.set(name, mlua::Nil);
    }

    // print 重定向到 tracing 日志 (任意数量参数拼接输出)。
    let print_fn = lua
        .create_function(|_, args: MultiValue| {
            let mut parts = Vec::with_capacity(args.len());
            for v in args.iter() {
                parts.push(v.to_string()?);
            }
            tracing::info!(target: "lua_strategy", "print: {}", parts.join(" "));
            Ok(())
        })
        .expect("create print fn");
    let _ = globals.set("print", print_fn);

    // 指令预算: 每 HOOK_GRANULARITY 条指令回调一次, 累计超过预算抛错中断。
    // fetch_sub 旧值 0 时立即回写 0 (saturating): 预算耗尽后持续抛错, 不回绕失效。
    let budget = SandboxBudget::new();
    let hook_budget = budget.0.clone();
    let hook = move |_lua: &Lua, _dbg: &mlua::Debug| -> mlua::Result<mlua::VmState> {
        let left = hook_budget.fetch_sub(1, Ordering::Relaxed);
        if left == 0 {
            hook_budget.store(0, Ordering::Relaxed);
            return Err(Error::RuntimeError(format!("指令预算超限 (>{INSTRUCTION_BUDGET} 条)")));
        }
        Ok(mlua::VmState::Continue)
    };
    lua.set_hook(HookTriggers::new().every_nth_instruction(HOOK_GRANULARITY), hook)
        .expect("set hook");

    (lua, budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> Lua {
        create_lua_sandbox().0
    }

    #[test]
    fn test_require_disabled() {
        // 无 package 库: require 为 nil, 调用报错。
        let lua = sandbox();
        let err = lua.load("require('os')").exec().unwrap_err();
        assert!(err.to_string().contains("nil"), "应报 nil 调用错误, got: {err}");
    }

    #[test]
    fn test_infinite_loop_intercepted() {
        let lua = sandbox();
        let err = lua.load("while true do end").exec().unwrap_err();
        assert!(err.to_string().contains("指令预算超限"), "死循环应被预算拦截, got: {err}");
    }

    #[test]
    fn test_budget_exhausted_stays_blocked() {
        // 回归: 预算耗尽后计数不得回绕 (saturating), 同一实例再次死循环仍被拦截。
        let (lua, budget) = create_lua_sandbox();
        // 第一次死循环: 耗尽预算。
        let err = lua.load("while true do end").exec().unwrap_err();
        assert!(err.to_string().contains("指令预算超限"));
        // 预算已耗尽 (未 reset): 第二次死循环必须仍被拦截。
        let err2 = lua.load("while true do end").exec().unwrap_err();
        assert!(
            err2.to_string().contains("指令预算超限"),
            "预算耗尽后应持续拦截, got: {err2}"
        );
        // reset 后恢复 (模拟下一 tick 开始)。
        budget.reset();
        let err3 = lua.load("while true do end").exec().unwrap_err();
        assert!(err3.to_string().contains("指令预算超限"), "reset 后应恢复拦截, got: {err3}");
    }

    #[test]
    fn test_memory_limit_blocks_huge_alloc() {
        // 回归: 单次 C 调用 (string.rep) 不得绕过指令预算造成 OOM。
        let lua = sandbox();
        // 100MB > 64MB 上限 → 报内存错误。
        let err = lua
            .load("local s = string.rep('x', 100 * 1024 * 1024) return #s")
            .eval::<usize>()
            .unwrap_err();
        assert!(
            err.to_string().contains("memory") || err.to_string().contains("not enough"),
            "超限分配应报内存错误, got: {err}"
        );
    }

    #[test]
    fn test_pcall_disabled() {
        // 回归: pcall/xpcall 必须移除 — 否则可捕获 hook 预算错误, 死循环 CPU 无限燃烧。
        let lua = sandbox();
        for name in ["pcall", "xpcall"] {
            let v: Option<mlua::Value> = lua.load(format!("return {name}")).eval().unwrap();
            assert!(v.is_none(), "{name} 应为 nil");
        }
    }

    #[test]
    fn test_infinite_loop_not_swallowed_by_pcall() {
        // 回归: pcall 包裹的死循环也必须被预算拦截 (pcall 已禁用)。
        let lua = sandbox();
        let err = lua
            .load("pcall(function() while true do end end)")
            .exec()
            .unwrap_err();
        assert!(
            err.to_string().contains("nil") || err.to_string().contains("指令预算超限"),
            "pcall 包裹死循环应被拦截, got: {err}"
        );
    }

    #[test]
    fn test_os_io_debug_not_loaded() {
        let lua = sandbox();
        // os/io/debug 库未加载 → 访问为 nil。
        for name in ["os", "io", "debug", "package", "coroutine"] {
            let v: Option<mlua::Value> = lua.load(format!("return {name}")).eval().unwrap();
            assert!(v.is_none(), "{name} 应为 nil");
        }
    }

    #[test]
    fn test_loadstring_disabled() {
        let lua = sandbox();
        // Lua 5.4: loadstring 本就不存在 (恒 nil); load 才是真正的动态加载入口。
        for name in ["load", "loadstring", "loadfile", "dofile"] {
            let err = lua.load(format!("{name}('x=1')")).exec().unwrap_err();
            assert!(err.to_string().contains("nil"), "{name} 应为 nil, got: {err}");
        }
    }

    #[test]
    fn test_print_redirected_no_panic() {
        // print 重定向到 tracing, 执行不报错、不写 stdout。
        let lua = sandbox();
        lua.load("print('hello', 42)").exec().unwrap();
    }

    #[test]
    fn test_safe_libs_available() {
        // 策略需要的常用函数仍可用。
        let lua = sandbox();
        let sum: i64 = lua.load("return math.floor(1.9) + string.len('abc')").eval().unwrap();
        assert_eq!(sum, 4);
    }
}
