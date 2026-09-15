//! 019 AI 助手真实链路冒烟(阶段一 T007)。
//!
//! 默认**不运行**(`#[ignore]`), 需手动指定本机/已配置的 LLM 端点:
//!
//! ```bash
//! # 推荐: 指向你配置好的正式通道(例如 DeepSeek; key 由 env 提供, 不落盘)
//! RICOW_AI_BASE_URL=https://api.deepseek.com/v1 RICOW_AI_MODEL=deepseek-chat \
//!   RICOW_AI_API_KEY=sk-xxx cargo test -p ricow --test ai_ollama_smoke -- --ignored
//! # 可选: 本机 Ollama(需自己先 `ollama pull <支持工具调用的模型>`; 无 API key)
//! RICOW_AI_BASE_URL=http://127.0.0.1:11434/v1 RICOW_AI_MODEL=<模型名> \
//!   cargo test -p ricow --test ai_ollama_smoke -- --ignored
//! ```
//!
//! 注: 默认值(本机 Ollama + qwen3:0.6b)是 019 开发期的链路验证环境 —— 该模型**只用于验证
//! "链路通不通", 其回答质量(编造日志行/数字失真)不足以面向用户, 且已在验证后从本机删除。
//!
//! 纪律: 不 mock —— 这里跑的是真实 HTTP 调用(CLI 二进制 → rig → 端点)。

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ricow")
}

#[test]
#[ignore = "需真实 LLM 端点(默认本机 Ollama; 见本文件头注释)"]
fn ai_single_shot_roundtrip_chinese() {
    let base_url =
        std::env::var("RICOW_AI_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:11434/v1".into());
    let model = std::env::var("RICOW_AI_MODEL").unwrap_or_else(|_| "qwen3:0.6b".into());

    // 独立数据目录: 只写 ai.toml, 不碰用户数据
    let root = std::env::temp_dir().join(format!("ricow-ai-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("建临时数据目录");
    std::fs::write(
        root.join("ai.toml"),
        format!("[ai]\nprovider = \"custom\"\nmodel = \"{model}\"\nbase_url = \"{base_url}\"\n"),
    )
    .expect("写 ai.toml");

    let out = Command::new(bin())
        .args(["ai", "请只回答两个字: 收到"])
        .env("RICOW_ROOT", &root)
        .output()
        .expect("运行 ricow ai");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    println!("--- stdout ---\n{stdout}\n--- stderr(尾部) ---\n{stderr}");

    assert!(out.status.success(), "真实调用应成功; exit={:?} stderr={stderr}", out.status.code());
    // 横幅存在(证明走的是内置 AI 通道)
    assert!(stdout.contains("ricow AI 助手"), "缺少会话横幅: {stdout}");
    // 回答非空且含中文(真实模型输出, 不校验具体内容)
    let answer: String = stdout
        .lines()
        .skip_while(|l| !l.starts_with("  上限:"))
        .skip(1)
        .collect::<Vec<_>>()
        .join("");
    assert!(
        answer.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).count() >= 2,
        "应得到中文回答: {stdout}"
    );
    // 日志不得污染 stdout(018/019 约定: 日志走 stderr; `ricow mcp` 的 stdout 是协议通道)
    assert!(!stdout.contains("INFO") && !stdout.contains("WARN"), "stdout 不应含日志行: {stdout}");

    let _ = std::fs::remove_dir_all(&root);
}
