#!/usr/bin/env bash
# ricow 安全 / 架构红线门禁 (grep 级, 零依赖, 三平台一致)
#
# 为什么用 grep 而不是单测?
#   这些红线是"结构性"的: 违反它们**不会**让任何单测变红 —— 功能照常工作, 却让安全模型
#   或架构铁律静默失守 (写操作绕过确认门、密钥进日志、引擎里长出策略参数…)。
#   单测覆盖"行为", 本脚本覆盖"形状"。成本趋近于零, 防回归极强。
#
# 用法:
#   bash scripts/ci_grep_gates.sh          # 退出码 0 = 全绿
#
# 新增红线时: 先想清楚"违反它的代价"与"误报概率"。宁可少一条, 不要留一条天天误报的。
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

FAILED=0
RED=$'\033[31m'
GREEN=$'\033[32m'
DIM=$'\033[2m'
RESET=$'\033[0m'
if [ ! -t 1 ]; then RED=''; GREEN=''; DIM=''; RESET=''; fi

# ---------------------------------------------------------------------------
# 工具: 只取"生产代码"行 (格式 `<文件>\t<行号>\t<内容>`)。
#
# 测试代码用 `#[cfg(test)]` 标注 (块级 `mod tests {}` 或单项 `fn helper()`), 这里按
# 花括号配平整块跳过 —— 红线只约束生产路径, 单测里写临时文件/打日志是正常操作。
# ---------------------------------------------------------------------------
non_test_lines() {
    awk '
        FNR == 1 { intest = 0; depth = 0; started = 0 }
        {
            if (intest == 0) {
                if ($0 ~ /^[[:space:]]*#\[cfg\(test\)\]/) {
                    intest = 1; depth = 0; started = 0
                    next
                }
                print FILENAME "\t" FNR "\t" $0
                next
            }
            # intest == 1: 整块跳过, 直到花括号配平 (或单个以 ; 结尾的属性项)
            o = gsub(/\{/, "{")
            c = gsub(/\}/, "}")
            depth += o - c
            if (o > 0) started = 1
            if (started == 1 && depth <= 0) intest = 0
            else if (started == 0 && o == 0 && $0 ~ /;/) intest = 0
        }
    ' "$@"
}

fail() {
    FAILED=1
    printf '%s\n' ""
    printf '%s[红线] %s%s\n' "$RED" "$1" "$RESET"
    printf '  %s\n' "$2"
    printf '%s\n' "$3" | sed 's/^/    /'
    printf '  %s\n' "修: 让生产路径回到红线内, 而不是改本脚本。"
}

pass() { printf '%s[ok]%s %s%s\n' "$GREEN" "$RESET" "$1" "$DIM$2$RESET"; }

ALL_SRC=$(find crates -type f -name '*.rs' -path '*/src/*' | sort)
AI_SRC=$(find crates/ricow/src/ai -type f -name '*.rs' 2>/dev/null | sort)

if [ -z "$ALL_SRC" ]; then
    printf '找不到任何源码 (crates/**/src/**/*.rs) —— 是否在仓库根运行?\n' >&2
    exit 2
fi

# ---------------------------------------------------------------------------
# 红线 1: AI 工具层零落盘。
#
# `crates/ricow/src/ai/` 是"模型能直接驱动的"那一层, 它只允许**提议**动作;
# 真正的落盘必须由宿主在用户确认后执行 (`ai/session.rs::execute_confirmed` → 引擎)。
# 一旦这层出现 fs::write / create_dir_all, 就等于给模型开了一条绕过确认门的写通道。
# ---------------------------------------------------------------------------
hits=$(non_test_lines $AI_SRC \
    | grep -E '(std::)?fs::(write|remove_file|remove_dir_all|rename|copy|create_dir|create_dir_all)[[:space:]]*\(|File::create[[:space:]]*\(|OpenOptions::new[[:space:]]*\(' \
    || true)
if [ -n "$hits" ]; then
    fail "AI 工具层不得直接落盘 (写操作必须经确认门)" \
        "crates/ricow/src/ai/ 的生产代码只允许提议动作; 落盘走 ai/session.rs::execute_confirmed。" \
        "$hits"
else
    pass "AI 工具层零落盘" "(crates/ricow/src/ai/, 排除 #[cfg(test)])"
fi

# ---------------------------------------------------------------------------
# 红线 2: 明文密钥不进日志 / 对话 sink。
#
# 密钥一旦进了 stdout / stderr / tracing, 就会落进日志文件、终端回滚缓冲、Web SSE 流。
# 打印"是否已配置"要用布尔量, 不要打印值本身 (哪怕只打印前几位)。
# ---------------------------------------------------------------------------
hits=$(non_test_lines $ALL_SRC \
    | grep -E '(println!|eprintln!|print!|writeln!|write!|(^|[^a-zA-Z_!])(trace|debug|info|warn|error)![[:space:]]*\(|tracing::(trace|debug|info|warn|error)!)' \
    | grep -iE 'api_?key|apikey|secret|password|private_?key' \
    || true)
if [ -n "$hits" ]; then
    fail "明文密钥不得进入日志 / 输出" \
        "打印是否已配置请用布尔量, 不要拼接密钥值 (api_key / secret / password …)。" \
        "$hits"
else
    pass "日志零密钥引用" "(全仓 src, 排除 #[cfg(test)])"
fi

# ---------------------------------------------------------------------------
# 红线 3: 主干不留调试残留。
# ---------------------------------------------------------------------------
hits=$(non_test_lines $ALL_SRC | grep -E '\bdbg!\(|\btodo!\(|\bunimplemented!\(' || true)
if [ -n "$hits" ]; then
    fail "主干不得残留 dbg! / todo!() / unimplemented!()" \
        "未实现的路径要么返回可读错误, 要么根本不该合入。" \
        "$hits"
else
    pass "无调试残留" "(dbg! / todo! / unimplemented!)"
fi

# ---------------------------------------------------------------------------
# 红线 4: 策略落盘只有一个入口。
#
# `execute_strategy` 会消费一次性 token 并写 strategies/*.toml|*.lua。
# 它只允许出现在两处调用点 (CLI `deploy` 与对话确认后的宿主执行) + 引擎内的定义处;
# 任何新调用点都意味着多了一条不受确认门约束的落盘路径。
# ---------------------------------------------------------------------------
# 用 awk 按第 1 列 (文件路径) 过滤而非 grep: `non_test_lines` 的分隔符是真正的 TAB,
# 而 ERE 里的 `\t` 不是转义 —— 用 grep 写这个白名单会静默失效 (只剩"看起来对")。
hits=$(non_test_lines $ALL_SRC | awk -F'\t' '
    $3 ~ /(^|[^a-zA-Z_])execute_strategy[[:space:]]*\(/ &&
    $1 != "crates/ricow/src/commands/deploy.rs" &&
    $1 != "crates/ricow/src/ai/session.rs" &&
    $1 != "crates/ricow_engine/src/strategy.rs" { print }')
if [ -n "$hits" ]; then
    fail "execute_strategy 只允许既有两处调用点 (+ 引擎定义处)" \
        "新调用点等于多一条绕过取消/确认门的落盘路径; 确有必要请同步更新本脚本的注释与白名单。" \
        "$hits"
else
    pass "落盘入口唯一" "(execute_strategy: deploy.rs / ai/session.rs / 定义处)"
fi

printf '%s\n' ""
if [ "$FAILED" -eq 0 ]; then
    printf '%s安全红线门禁: 全绿%s\n' "$GREEN" "$RESET"
    exit 0
fi
printf '%s安全红线门禁: 未通过 (见上)%s\n' "$RED" "$RESET"
exit 1
