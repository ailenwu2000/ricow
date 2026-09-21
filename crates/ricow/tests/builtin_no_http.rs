//! 028 T036: 内置脚本零网络断言 —— `strategies/builtin/**/*.lua` 不得出现 `http.`。
//!
//! 为什么要把"内置不用网络"钉成断言: 项目自带策略只用平台数据服务(可复现、可审计);
//! 用户自写策略才允许自取任意 URL(028 D1: 平台不限制用户策略的网络访问, 用户自担)。
//! 一旦内置脚本悄悄用上网络, 回测/复现的前提就塌了 —— 这条断言就是那道闸。

use std::fs;
use std::path::{Path, PathBuf};

/// 递归收集 `.lua` 文件(不依赖 walkdir, 保持依赖面不变)。
fn collect_lua(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lua(&path, out);
        } else if path.extension().map(|e| e == "lua").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// 去掉行注释后是否出现 `http.` / `http:`(注释里提到不算违规)。
fn uses_http(line: &str) -> bool {
    let code = match line.find("--") {
        Some(i) => &line[..i],
        None => line,
    };
    code.contains("http.") || code.contains("http:")
}

#[test]
fn builtin_no_http() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../strategies/builtin");
    let mut files = Vec::new();
    collect_lua(&root, &mut files);
    assert!(!files.is_empty(), "内置脚本目录应有 .lua 文件: {}", root.display());

    let mut offenders = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("读取内置脚本");
        for (no, line) in text.lines().enumerate() {
            if uses_http(line) {
                offenders.push(format!("{}:{}: {}", file.display(), no + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "内置脚本禁止直接使用网络(平台数据服务之外的取数不可复现):\n{}",
        offenders.join("\n")
    );
}
