//! `strategy_io` 的端点/纯函数测试(036 拆文件下沉): 逻辑在 `super`, 这里只组织用例。

use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use ricow_strategy::Database;

// ---- 纯函数 ----

#[test]
fn test_parse_market_whitelist() {
    assert_eq!(parse_market("spot").unwrap(), "spot");
    assert_eq!(parse_market(" FUTURES ").unwrap(), "futures");
    for bad in ["", "  ", "fx", "spots", "现货"] {
        assert!(parse_market(bad).is_err(), "market={bad:?} 应拒绝");
    }
    let err = parse_market("options").unwrap_err();
    assert!(err.contains("spot|futures") && err.contains("options"), "{err}");
}

#[test]
fn test_config_values_converts_json_types() {
    let mut params = HashMap::new();
    params.insert("a_f".to_string(), serde_json::json!(0.01));
    params.insert("a_i".to_string(), serde_json::json!(14));
    params.insert("a_s".to_string(), serde_json::json!("u"));
    params.insert("a_b".to_string(), serde_json::json!(true));
    let cv = config_values(&params).unwrap();
    assert!(matches!(cv.get("a_f"), Some(ConfigValue::Float(f)) if (*f - 0.01).abs() < 1e-9));
    assert!(matches!(cv.get("a_i"), Some(ConfigValue::Integer(14))));
    assert!(matches!(cv.get("a_s"), Some(ConfigValue::String(s)) if s == "u"));
    assert!(matches!(cv.get("a_b"), Some(ConfigValue::Boolean(true))));

    // null / 对象 / 数组拒绝, 且错误带参数键。
    let mut bad = HashMap::new();
    bad.insert("k".to_string(), Value::Null);
    assert!(config_values(&bad).unwrap_err().contains('k'));
    bad.insert("k".to_string(), serde_json::json!({"x": 1}));
    assert!(config_values(&bad).unwrap_err().contains('k'));
    bad.insert("k".to_string(), serde_json::json!([1]));
    assert!(config_values(&bad).unwrap_err().contains('k'));
}

#[test]
fn test_parse_lua_line_finds_chunk_line() {
    let err =
        "生成代码未通过编译门禁:\n脚本编译错误:\n[string \"chunk\"]:12: '=' expected near 'end'";
    assert_eq!(parse_lua_line(err), Some(12));
    assert_eq!(parse_lua_line("无行号信息"), None);
    // 多个命中取最后一个(消息正文里偶发的 :2: 不影响定位)。
    assert_eq!(parse_lua_line("x:2: boom [string \"c\"]:7: near"), Some(7));
}

#[test]
fn test_decide_allow_replace_matrix() {
    // 新名 + 无冲突 + 未运行 → 全新部署(allow_replace=false)。
    assert!(!decide_allow_replace("new-grid", &[], false, false, false).unwrap());

    // ① 名格式非法 → 400 invalid_name。
    let r = decide_allow_replace("网格 A", &[], false, false, false).unwrap_err();
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.code, "invalid_name");

    // ② 新名与既有策略互为前缀(两个方向)→ 409 prefix, 文案复用 CLI 原文。
    let existing = vec!["eth-grid".to_string()];
    let r = decide_allow_replace("eth-grid-300", &existing, false, false, false).unwrap_err();
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.code, "prefix");
    assert!(r.message.contains("互为前缀"), "{r:?}");
    assert!(decide_allow_replace("eth", &existing, false, false, false).is_err());

    // ③ 内置保留名 → 409 reserved(即使 overwrite 也拒)。
    let r = decide_allow_replace("paired_grid", &[], false, true, false).unwrap_err();
    assert_eq!(r.code, "reserved");

    // ④ 运行中实例 → 409 running(优先级高于 overwrite)。
    let r = decide_allow_replace("live-one", &["x".to_string()], true, true, true).unwrap_err();
    assert_eq!(r.code, "running");

    // ⑤ 已存在 + overwrite=false → 409 exists; true → 受控覆盖。
    let r = decide_allow_replace("old-one", &[], true, false, false).unwrap_err();
    assert_eq!(r.code, "exists");
    assert!(decide_allow_replace("old-one", &[], true, true, false).unwrap());

    // 覆盖保存跳过前缀自冲突: 自己在 existing 里也必须放行(只要未运行)。
    assert!(decide_allow_replace("eth-grid", &["eth-grid".to_string()], true, true, false).unwrap());
}

// ---- 端到端(真实回环套接字, 全程离线: 编译门禁是本地 mlua, 不触网) ----

/// 临时数据目录。
fn tmp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ricow-web-stratio-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("建临时数据目录");
    d
}

/// 起真实监听服务(中间件 + 路由一体), 返回端口。
async fn boot(root: PathBuf) -> u16 {
    let db = Database::open_in_memory().await.expect("开内存库");
    let store = super::super::SessionStore::new(db.clone());
    let starter: super::super::Starter =
        Arc::new(|_id: &str, _sink: &mut super::super::WebSink| Ok(()));
    let state = super::super::WebState::new("tok-ok".to_string(), root, db, store, starter);
    let (listener, port) = super::super::bind(0).await.expect("绑定回环端口");
    tokio::spawn(super::super::serve(listener, state));
    port
}

/// 裸 HTTP/1.1 请求(本 crate 无 HTTP 客户端依赖), 回完整响应文本。
async fn raw(port: u16, method: &str, target: &str, body: Option<&str>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream =
        tokio::net::TcpStream::connect((super::super::BIND_ADDR, port)).await.expect("连上服务");
    let head = match body {
        Some(b) => format!(
            "{method} {target} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{b}",
            super::super::BIND_ADDR,
            b.len()
        ),
        None => format!(
            "{method} {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            super::super::BIND_ADDR
        ),
    };
    stream.write_all(head.as_bytes()).await.expect("发请求");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("读响应");
    String::from_utf8_lossy(&buf).to_string()
}

fn status_of(res: &str) -> u16 {
    res.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn body_of(res: &str) -> &str {
    res.split_once("\r\n\r\n").map_or("", |(_, b)| b)
}

/// 401 矩阵: 源码读 + 保存写都在 token 门禁之后 —— 无/错 token 一律 401 空体,
/// 且不回显请求里的 Lua 源码(照 markets.rs 离线风格: 401 在中间件短路)。
#[tokio::test]
async fn test_strategy_endpoints_require_token_offline() {
    let root = tmp_root("auth");
    let port = boot(root).await;

    const MARK: &str = "LUA-SECRET-MARK-on_tick";
    let post_body = serde_json::json!({
        "name": "auth-probe", "market": "spot", "pair": "ETHUSDT", "code": MARK
    })
    .to_string();

    for probe in [
        "/api/strategies/paired_grid/source".to_string(),
        "/api/strategies/paired_grid/source?token=wrong".to_string(),
        "/api/strategies".to_string(),
        "/api/strategies?token=wrong".to_string(),
    ] {
        let res = if probe.starts_with("/api/strategies?") || probe == "/api/strategies" {
            raw(port, "POST", &probe, Some(&post_body)).await
        } else {
            raw(port, "GET", &probe, None).await
        };
        assert!(res.starts_with("HTTP/1.1 401"), "{probe} 应 401, 实际: {res}");
        assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe}");
        assert!(!res.contains(MARK), "401 不得回显请求源码: {res}");
    }

    // AI 改 Lua 同样在 token 门禁之后: 无/错 token 401 空体, 不回显指令。
    let ai_body = format!(r#"{{"instruction":"{MARK}"}}"#);
    for probe in [
        "/api/strategies/paired_grid/ai-edit".to_string(),
        "/api/strategies/paired_grid/ai-edit?token=wrong".to_string(),
    ] {
        let res = raw(port, "POST", &probe, Some(&ai_body)).await;
        assert!(res.starts_with("HTTP/1.1 401"), "{probe} 应 401, 实际: {res}");
        assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe}");
        assert!(!res.contains(MARK), "401 不得回显修改指令: {res}");
    }

    // AI 从零生成(P0-3)同样在 token 门禁之后。
    let gen_body =
        format!(r#"{{"name":"auth-probe","market":"spot","pair":"ETHUSDT","idea":"{MARK}"}}"#);
    for probe in [
        "/api/strategies/ai-generate".to_string(),
        "/api/strategies/ai-generate?token=wrong".to_string(),
    ] {
        let res = raw(port, "POST", &probe, Some(&gen_body)).await;
        assert!(res.starts_with("HTTP/1.1 401"), "{probe} 应 401, 实际: {res}");
        assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe}");
        assert!(!res.contains(MARK), "401 不得回显生成意图: {res}");
    }

    // 用户策略清单保存 (P2-8) 同样在 token 门禁之后。
    let mf_body = format!(r#"{{"name":"{MARK}","params":[]}}"#);
    for probe in [
        "/api/strategies/paired_grid/manifest".to_string(),
        "/api/strategies/paired_grid/manifest?token=wrong".to_string(),
    ] {
        let res = raw(port, "POST", &probe, Some(&mf_body)).await;
        assert!(res.starts_with("HTTP/1.1 401"), "{probe} 应 401, 实际: {res}");
        assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe}");
        assert!(!res.contains(MARK), "401 不得回显清单内容: {res}");
    }

    // 对 token 取得到内置源码(证明拦的不是"路由不存在")。
    let res = raw(port, "GET", "/api/strategies/paired_grid/source?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200);
    assert!(body_of(&res).contains("on_tick"));
}

/// GET source: 内置原文 + instance_toml=null; 未知 id / 目录穿越形态一律 404 中文。
#[tokio::test]
async fn test_get_source_builtin_unknown_and_traversal_404() {
    let root = tmp_root("source");
    let port = boot(root).await;

    let res = raw(port, "GET", "/api/strategies/shannon_spot_grid/source?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""id":"shannon_spot_grid""#), "{body}");
    assert!(body.contains(r#""market":"spot""#), "{body}");
    assert!(body.contains("on_tick"), "lua 为文件原文: {body}");
    assert!(body.contains(r#""instance_toml":null"#), "内置策略实例 TOML 必须为 null: {body}");

    // 未知 id → 404 中文。
    let res = raw(port, "GET", "/api/strategies/no-such-xyz/source?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 404);
    assert!(body_of(&res).contains("没有策略"), "404 要中文可读: {res}");

    // 目录穿越 / 路径分隔 → 一律 404(不接受 ../、斜杠), 不回显路径。
    for bad in ["..%2Fevil", "a%2Fb", "..%5Cevil"] {
        let target = format!("/api/strategies/{bad}/source?token=tok-ok");
        let res = raw(port, "GET", &target, None).await;
        assert_eq!(status_of(&res), 404, "{} 应 404: {res}", bad);
        assert!(!body_of(&res).contains("strategies"), "不得回显路径: {res}");
    }
}

/// POST 保存全链路: 新建 200 落盘 → GET 源码可回读 → 同名 409 → 编译失败 400 且旧文件不坏
/// → 受控覆盖 200 且有 .bak → 保留名/前缀/坏市场/坏名各自的拒绝码。
// ② 处有意持 ENV_LOCK 跨 await: catalog 扫描读进程级 RICOW_ROOT, 必须串行化,
// 否则并行测试会读到彼此的环境变量(同 commands/backtest.rs 的 ENV_LOCK 范式)。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_post_save_roundtrip_compile_guard_and_conflicts() {
    let root = tmp_root("save");
    let port = boot(root.clone()).await;
    let code = "function on_tick(ctx)\n    return {}\nend\n";
    let post = |name: &str, market: &str, the_code: &str, overwrite: bool| {
        serde_json::json!({
            "name": name,
            "market": market,
            "pair": "ETHUSDT",
            "code": the_code,
            "params": {"order_size": 0.01, "grid_count": 14, "mode": "u", "dry": true},
            "overwrite": overwrite,
        })
        .to_string()
    };

    // ① 新名保存 → 200, 文件落位; TOML 只引用 script_path, 不内嵌 Lua。
    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(&post("rt-save", "spot", code, false)),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""name":"rt-save""#) && body.contains(r#""market":"spot""#), "{body}");
    assert!(body.contains("rt-save.toml") && body.contains("rt-save.lua"), "{body}");
    assert!(!body.contains(r#""backup""#), "全新部署按契约 backup? 字段缺省: {body}");
    let toml_path = root.join("strategies").join("rt-save.toml");
    let lua_path = root.join("strategies").join("spot").join("rt-save.lua");
    assert!(toml_path.is_file() && lua_path.is_file());
    let toml_text = std::fs::read_to_string(&toml_path).unwrap();
    assert!(toml_text.contains("script_path") && toml_text.contains("spot/rt-save.lua"));
    assert!(!toml_text.contains("function on_tick"), "Lua 不得内嵌进 TOML");

    // ② GET source 能回读用户策略的实例 TOML 原文(目录扫描走 project_root → 临时设 RICOW_ROOT)。
    {
        let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
        std::env::set_var("RICOW_ROOT", &root);
        let res = raw(port, "GET", "/api/strategies/rt-save/source?token=tok-ok", None).await;
        assert_eq!(status_of(&res), 200, "{res}");
        let body = body_of(&res);
        assert!(body.contains(r#""instance_toml":"#) && !body.contains(r#""instance_toml":null"#));
        assert!(body.contains("script_path"), "实例 TOML 原文应在响应里: {body}");
        assert!(body.contains("return {}"), "lua 原文应在响应里: {body}");
        std::env::remove_var("RICOW_ROOT");
    }

    // ③ 同名再存不带 overwrite → 409 exists。
    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(&post("rt-save", "spot", code, false)),
    )
    .await;
    assert_eq!(status_of(&res), 409, "{res}");
    assert!(body_of(&res).contains(r#""code":"exists""#), "{res}");

    // ④ 编译失败: overwrite=true 也必须在落盘前 400 compile, 带行号, 旧文件一字节不变、无备份。
    let old_lua = std::fs::read_to_string(&lua_path).unwrap();
    let bad_code = "function on_tick(ctx)\n    local x = =\nend\n";
    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(&post("rt-save", "spot", bad_code, true)),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""code":"compile""#), "{body}");
    assert!(body.contains(r#""line":2"#), "应透传 mlua 行号: {body}");
    assert_eq!(std::fs::read_to_string(&lua_path).unwrap(), old_lua, "编译失败不得破坏旧 Lua");
    for ent in walkdir(&root.join("strategies")) {
        assert!(
            ent.extension().and_then(|s| s.to_str()) != Some("bak"),
            "编译失败不得产生备份: {}",
            ent.display()
        );
    }

    // ⑤ 受控覆盖(合法新代码)→ 200, backup 非空; 磁盘有 .bak 且内容是旧脚本, 新脚本已替换。
    let code2 = "function on_tick(ctx)\n    return { v = 2 }\nend\n";
    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(&post("rt-save", "spot", code2, true)),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""backup":"#), "受控覆盖应给 backup 字段: {body}");
    assert!(body.contains(".bak"), "backup 应指向 .bak: {body}");
    assert!(std::fs::read_to_string(&lua_path).unwrap().contains("v = 2"));
    let has_old_backup = walkdir(&root.join("strategies")).iter().any(|p| {
        p.extension().and_then(|s| s.to_str()) == Some("bak")
            && p.to_string_lossy().contains("rt-save.lua.")
            && std::fs::read_to_string(p).map(|t| t.contains("return {}")).unwrap_or(false)
    });
    assert!(has_old_backup, "旧 Lua 必须备份成 .bak 且内容为旧文");

    // ⑥ 保留名 → 409 reserved。
    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(&post("paired_grid", "spot", code, false)),
    )
    .await;
    assert_eq!(status_of(&res), 409, "{res}");
    assert!(body_of(&res).contains(r#""code":"reserved""#), "{res}");

    // ⑦ 与既有 rt-save 互为前缀的新名 → 409 prefix(两个方向都拒)。
    for candidate in ["rt-save-2", "rt"] {
        let res = raw(
            port,
            "POST",
            "/api/strategies?token=tok-ok",
            Some(&post(candidate, "spot", code, false)),
        )
        .await;
        assert_eq!(status_of(&res), 409, "{candidate} 应 409: {res}");
        assert!(body_of(&res).contains(r#""code":"prefix""#), "{candidate}: {res}");
    }

    // ⑧ market 非法 → 400; 名格式非法 → 400 invalid_name; 参数值非法 → 400 带键名。
    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(&post("ok-other", "options", code, false)),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("spot|futures"), "{res}");

    let mut bad_name =
        serde_json::json!({"name": "网格 A", "market": "spot", "pair": "ETHUSDT", "code": code});
    bad_name["params"] = serde_json::json!({"weird": null});
    let res = raw(port, "POST", "/api/strategies?token=tok-ok", Some(&bad_name.to_string())).await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains(r#""code":"invalid_name""#), "名格式校验先于参数校验: {res}");

    let mut bad_param =
        serde_json::json!({"name": "ok-param", "market": "spot", "pair": "ETHUSDT", "code": code});
    bad_param["params"] = serde_json::json!({"weird": null});
    let res = raw(port, "POST", "/api/strategies?token=tok-ok", Some(&bad_param.to_string())).await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("weird"), "参数错误要带键名: {res}");

    let _ = std::fs::remove_dir_all(&root);
}

/// manifest 参数摘要: 键名/类型/必填/枚举/默认都要来自清单数据(不硬编码参数名)。
#[test]
fn test_manifest_summary_renders_schema() {
    let entry = catalog::find("shannon_spot_grid").expect("内置清单 shannon_spot_grid 必须存在");
    let s = manifest_summary(&entry.manifest);
    assert!(s.contains(&entry.manifest.id) && s.contains(&entry.manifest.name), "{s}");
    assert!(s.contains("键名") && s.contains("类型"), "{s}");
    // 内置清单至少有一个参数, 每个参数行都要给键名; 且摘要里找得到 f64/string/bool 之一。
    assert!(!entry.manifest.params.is_empty());
    assert!(
        s.contains("f64") || s.contains("i64") || s.contains("string") || s.contains("bool"),
        "类型标注缺失: {s}"
    );
    assert!(
        entry.manifest.params.iter().any(|p| s.contains(&p.key) && s.contains(&p.name)),
        "每个参数的键名/中文名至少命中一个: {s}"
    );
}

/// AI 改 Lua 离线路径: 空指令 400 / 未知 id 404 / 未配 AI 密钥 403 need_keys(在联网前失败)。
///
/// 成功的 LLM 往返与编译门禁的"AI 产物编译失败"分支需要真实上游, 按任务约定不进单测;
/// 编译门禁本身已由保存端到端用例(真实 mlua)覆盖。
// 有意持 ENV_LOCK 跨 await: 临时清掉 RICOW_AI_* 防止本机环境变量把缺密钥路径变成真实联网。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_ai_edit_validation_and_need_keys_offline() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    // 保存并清空三个 AI 覆盖变量(测完还原), 确保走到"远程端点缺密钥"分支。
    let saved = ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"]
        .map(std::env::var)
        .map(|r| r.ok());
    for k in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"] {
        std::env::remove_var(k);
    }

    let root = tmp_root("aiedit");
    let port = boot(root.clone()).await;
    let post = |ins: &str| format!(r#"{{"instruction":"{ins}"}}"#);

    // ① 空 / 纯空白指令 → 400(不触网、不查策略)。
    let res = raw(
        port,
        "POST",
        "/api/strategies/shannon_spot_grid/ai-edit?token=tok-ok",
        Some(&post("   ")),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("instruction"), "{res}");

    // ② 非 JSON → 400。
    let res = raw(
        port,
        "POST",
        "/api/strategies/shannon_spot_grid/ai-edit?token=tok-ok",
        Some("{ not json"),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");

    // ③ 未知 id → 404(与 source 同口径, 先于任何 AI 调用)。
    let res = raw(
        port,
        "POST",
        "/api/strategies/no-such-xyz/ai-edit?token=tok-ok",
        Some(&post("随便改")),
    )
    .await;
    assert_eq!(status_of(&res), 404, "{res}");
    assert!(body_of(&res).contains("没有策略"), "{res}");

    // ④ 合法指令 + 内置策略 + 空配置(临时 root, 模板无 api_key, 默认 deepseek 远程端点)
    //    → 403 code=need_keys, 发生在网络请求之前; 且不回显指令原文。
    let instruction = "UNIQUE-INSTRUCTION-把网格间距改成 ATR 三倍";
    let res = raw(
        port,
        "POST",
        "/api/strategies/shannon_spot_grid/ai-edit?token=tok-ok",
        Some(&post(instruction)),
    )
    .await;
    assert_eq!(status_of(&res), 403, "缺密钥应 403: {res}");
    let body = body_of(&res);
    assert!(body.contains(r#""code":"need_keys""#), "{body}");
    assert!(body.contains("密钥"), "403 要中文可读并引导配置: {body}");
    assert!(!body.contains("UNIQUE-INSTRUCTION"), "错误体不得回显指令/prompt: {body}");

    // 还原环境变量。
    for (k, v) in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].iter().zip(saved) {
        if let Some(val) = v {
            std::env::set_var(k, val);
        } else {
            std::env::remove_var(k);
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// GET 详情: 内置 → pair/current 皆 null(清单字段扁平保留); 用户实例 → pair 与已保存参数
/// 以 JSON 给出, 且内部键 script/script_path 不泄漏进 current(032 T027, 前端免解析 TOML)。
// 有意持 ENV_LOCK 跨 await: catalog 用户策略扫描读进程级 RICOW_ROOT, 与保存用例同款串行化。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_get_detail_carries_instance_current_values() {
    let root = tmp_root("detail");
    let port = boot(root.clone()).await;

    // ① 内置: 清单字段照常(对话视图依赖扁平的 name/params), pair/current 必须为 null。
    let res = raw(port, "GET", "/api/strategies/shannon_spot_grid?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""id":"shannon_spot_grid""#), "{body}");
    assert!(
        body.contains(r#""name":"#) && body.contains(r#""params":"#),
        "清单字段须扁平保留: {body}"
    );
    assert!(body.contains(r#""pair":null"#), "内置无实例 pair 必须为 null: {body}");
    assert!(body.contains(r#""current":null"#), "内置无实例 current 必须为 null: {body}");

    // ② 保存一个用户策略(带参数), 再以同一 RICOW_ROOT 取详情。
    let code = "function on_tick(ctx)\n    return {}\nend\n";
    let post_body = serde_json::json!({
        "name": "rt-detail",
        "market": "spot",
        "pair": "ETHUSDT",
        "code": code,
        "params": {"order_size": 0.01, "grid_count": 14, "mode": "u", "dry": true},
        "overwrite": false,
    })
    .to_string();
    let res = raw(port, "POST", "/api/strategies?token=tok-ok", Some(&post_body)).await;
    assert_eq!(status_of(&res), 200, "{res}");

    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    std::env::set_var("RICOW_ROOT", &root);
    let res = raw(port, "GET", "/api/strategies/rt-detail?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""pair":"ETHUSDT""#), "pair 应来自实例 TOML: {body}");
    // 当前参数以 JSON 基本类型给出(ConfigValue 无标签序列化)。
    assert!(body.contains(r#""order_size":0.01"#), "{body}");
    assert!(body.contains(r#""grid_count":14"#), "{body}");
    assert!(body.contains(r#""mode":"u""#), "{body}");
    assert!(body.contains(r#""dry":true"#), "{body}");
    // 内部键不得出现在 current(会被前端误当策略参数回存)。
    assert!(!body.contains(r#""script_path""#), "current 须剔除 script_path: {body}");
    std::env::remove_var("RICOW_ROOT");

    let _ = std::fs::remove_dir_all(&root);
}

/// 静态检查(P1-5)纯函数: 已知高频踩坑逐条命中; 干净代码零提示。
#[test]
fn test_lint_lua_flags_known_pitfalls() {
    // 干净代码: 无任何提示。
    assert!(super::lint_lua("local x = 1\nreturn {}\n").is_empty());

    let cases = [
        ("  a = fill.price", "fill_price"),
        ("  a = fill.size", "fill_size"),
        ("  local n = ctx:config_f64(\"x\") or 5", "or 缺省"),
        ("  os.time()", "沙箱"),
        ("  local o = { type = \"limit\", pair = p }", "order_type"),
        ("  while true do x = x + 1 end", "退出条件"),
    ];
    for (src, needle) in cases {
        let w = super::lint_lua(src);
        assert!(w.iter().any(|m| m.contains(needle)), "应命中 '{needle}', 实际: {w:?}");
    }
    // fill_size 是正解, 不该被 fill.size 误报。
    assert!(super::lint_lua("a = fill.fill_size\n").is_empty());
    // order_type 正解不误报。
    assert!(super::lint_lua("o = { order_type = \"limit\" }\n").is_empty());
}

/// AI 从零生成(P0-3)的同步校验: 校验全部发生在任何 AI 调用之前(全程离线)。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_ai_generate_validation_offline() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let saved = ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"]
        .map(std::env::var)
        .map(|r| r.ok());
    for k in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"] {
        std::env::remove_var(k);
    }

    let root = tmp_root("aigen");
    let port = boot(root.clone()).await;
    let post = |json: String| async move {
        raw(port, "POST", "/api/strategies/ai-generate?token=tok-ok", Some(&json)).await
    };

    // ① 非法名 → 400 code=invalid_name。
    let res =
        post(r#"{"name":"../evil","market":"spot","pair":"ETHUSDT","idea":"随便"}"#.to_string())
            .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains(r#""code":"invalid_name""#), "{res}");

    // ② 内置保留名 → 409 code=reserved。
    let res = post(
        r#"{"name":"shannon_spot_grid","market":"spot","pair":"ETHUSDT","idea":"随便"}"#
            .to_string(),
    )
    .await;
    assert_eq!(status_of(&res), 409, "{res}");
    assert!(body_of(&res).contains(r#""code":"reserved""#), "{res}");

    // ③ market 非法 → 400。
    let res =
        post(r#"{"name":"gen-x","market":"fx","pair":"ETHUSDT","idea":"随便"}"#.to_string()).await;
    assert_eq!(status_of(&res), 400, "{res}");

    // ④ pair 空 → 400。
    let res =
        post(r#"{"name":"gen-x","market":"spot","pair":"  ","idea":"随便"}"#.to_string()).await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("pair"), "{res}");

    // ⑤ idea 空 → 400(且先于任何 AI 调用)。
    let res =
        post(r#"{"name":"gen-x","market":"spot","pair":"ETHUSDT","idea":"   "}"#.to_string()).await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("idea"), "{res}");

    // ⑥ 非 JSON → 400。
    let res = post("{oops".to_string()).await;
    assert_eq!(status_of(&res), 400, "{res}");

    // ⑦ 合法意图 + 空配置(临时 root, 无 api_key, 默认远程端点)→ 403 need_keys,
    //    发生在网络请求之前; 且不回显 idea 原文。
    let idea = "UNIQUE-IDEA-双均线金叉做多";
    let body = format!(
        r#"{{"name":"gen-x","market":"spot","pair":"ETHUSDT","interval":"1h","idea":"{idea}"}}"#
    );
    let res = post(body).await;
    assert_eq!(status_of(&res), 403, "缺密钥应 403: {res}");
    let b = body_of(&res);
    assert!(b.contains(r#""code":"need_keys""#), "{b}");
    assert!(b.contains("密钥"), "403 要中文可读并引导配置: {b}");
    assert!(!b.contains("UNIQUE-IDEA"), "错误体不得回显 idea/prompt: {b}");

    for (k, v) in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].iter().zip(saved) {
        if let Some(val) = v {
            std::env::set_var(k, val);
        } else {
            std::env::remove_var(k);
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// 清单构造/校验 (P2-8, 纯函数): id/market 由调用方注入, 参数 schema 逐项校验。
#[test]
fn test_build_manifest_validation() {
    let ok = |params: serde_json::Value| ManifestRequest {
        name: "我的策略".into(),
        summary: "说明".into(),
        description: String::new(),
        suitable: String::new(),
        unsuitable: String::new(),
        params: serde_json::from_value(params).unwrap(),
    };

    // 合法: f64 必填 + enum(默认在 options 内) + bool。
    let m = build_manifest(
        "my-grid",
        "spot",
        ok(serde_json::json!([
            {"key": "start_price", "name": "开始价格", "type": "f64", "desc": "低于它激活", "required": true},
            {"key": "mode", "name": "模式", "type": "enum", "desc": "成交模式", "default": "u", "options": ["u", "coin"]},
            {"key": "dry", "name": "演练", "type": "bool", "desc": "只演练", "default": true}
        ])),
    )
    .unwrap();
    assert_eq!(m.id, "my-grid");
    assert_eq!(m.market, "spot");
    assert_eq!(m.params.len(), 3);
    // 空 description → None (不写该字段)。
    assert!(m.description.is_none());
    assert!(m.params[0].required);

    // 非法矩阵: 每项都带可读中文原因。
    let cases: Vec<(serde_json::Value, &str)> = vec![
        (
            serde_json::json!([{"key": "9bad key", "name": "n", "type": "f64", "desc": "d"}]),
            "9bad key",
        ),
        (serde_json::json!([{"key": "script", "name": "n", "type": "f64", "desc": "d"}]), "保留键"),
        (serde_json::json!([{"key": "a", "name": "", "type": "f64", "desc": "d"}]), "缺中文名"),
        (serde_json::json!([{"key": "a", "name": "n", "type": "f64", "desc": "  "}]), "缺说明"),
        (
            serde_json::json!([
                {"key": "a", "name": "n", "type": "f64", "desc": "d"},
                {"key": "a", "name": "m", "type": "i64", "desc": "d"}
            ]),
            "重复",
        ),
        (serde_json::json!([{"key": "e", "name": "n", "type": "enum", "desc": "d"}]), "options"),
        (
            serde_json::json!([{"key": "e", "name": "n", "type": "enum", "desc": "d", "options": ["u"], "default": "z"}]),
            "不在 options",
        ),
        (
            serde_json::json!([{"key": "n", "name": "n", "type": "f64", "desc": "d", "default": "abc"}]),
            "默认值类型",
        ),
        (
            serde_json::json!([{"key": "n", "name": "n", "type": "i64", "desc": "d", "default": 1.5}]),
            "默认值类型",
        ),
    ];
    for (params, needle) in cases {
        let err = build_manifest("my-grid", "spot", ok(params)).unwrap_err();
        assert!(err.contains(needle), "应命中 '{needle}', 实际: {err}");
    }

    // name 空 → 拒绝。
    let mut bad = ok(serde_json::json!([]));
    bad.name = "   ".into();
    assert!(build_manifest("my-grid", "spot", bad).unwrap_err().contains("中文名"));
}

/// 清单保存 (P2-8) 端到端: 落盘 strategies/{market}/{id}.toml → 详情 params 点亮;
/// 内置 → 409 builtin; 未知 → 404; 覆盖 → 备份。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_manifest_save_roundtrip_and_guards() {
    // catalog 扫描走 project_root(读 RICOW_ROOT), 与 state.root 指向同一临时目录。
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let root = tmp_root("manifest");
    std::env::set_var("RICOW_ROOT", &root);
    let port = boot(root.clone()).await;

    let code = "function on_tick(ctx)\n    return {}\nend\n";
    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(
            &serde_json::json!({"name": "mf-user", "market": "spot", "pair": "ETHUSDT", "code": code})
                .to_string(),
        ),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");

    let manifest = serde_json::json!({
        "name": "我的网格",
        "summary": "一句话说明",
        "suitable": "震荡市",
        "params": [
            {"key": "start_price", "name": "开始价格", "type": "f64", "desc": "低于它激活", "required": true},
            {"key": "mode", "name": "模式", "type": "enum", "desc": "成交模式", "default": "u", "options": ["u", "coin"]}
        ]
    });

    // ① 首次保存 → 200, 无 backup 字段; 落盘在 strategies/spot/mf-user.toml。
    let res = raw(
        port,
        "POST",
        "/api/strategies/mf-user/manifest?token=tok-ok",
        Some(&manifest.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""id":"mf-user""#) && body.contains(r#""market":"spot""#), "{body}");
    assert!(!body.contains(r#""backup""#), "首次创建无备份: {body}");
    let path = root.join("strategies").join("spot").join("mf-user.toml");
    assert!(path.is_file(), "清单须落盘到 {} ", path.display());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("start_price") && text.contains("options"), "{text}");
    // 落盘清单能被 catalog 解析(否则详情会静默丢失)。
    assert!(catalog::find("mf-user").is_some(), "保存后 catalog 应能扫到该策略");

    // ② 详情 params 点亮(参数表单据此渲染)。
    let res = raw(port, "GET", "/api/strategies/mf-user?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""key":"start_price""#), "详情应带清单参数: {body}");
    assert!(body.contains("低于它激活"), "{body}");

    // ③ 覆盖保存 → 出现 backup 字段, 且磁盘有 .bak。
    let res = raw(
        port,
        "POST",
        "/api/strategies/mf-user/manifest?token=tok-ok",
        Some(&manifest.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");
    assert!(body_of(&res).contains(r#""backup":"#), "覆盖应给备份: {res}");
    assert!(
        walkdir(&root.join("strategies").join("spot"))
            .iter()
            .any(|p| p.extension().and_then(|s| s.to_str()) == Some("bak")),
        "覆盖前须备份旧清单"
    );

    // ④ 内置策略 → 409 builtin(清单随代码发布)。
    let res = raw(
        port,
        "POST",
        "/api/strategies/paired_grid/manifest?token=tok-ok",
        Some(&manifest.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 409, "{res}");
    assert!(body_of(&res).contains(r#""code":"builtin""#), "{res}");

    // ⑤ 未知 id → 404。
    let res = raw(
        port,
        "POST",
        "/api/strategies/no-such/manifest?token=tok-ok",
        Some(&manifest.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 404, "{res}");

    // ⑥ 参数校验失败 → 400 且带问题键名, 不落盘(文件内容不变)。
    let before = std::fs::read_to_string(&path).unwrap();
    let bad = serde_json::json!({
        "name": "x",
        "params": [{"key": "9bad key", "name": "n", "type": "f64", "desc": "d"}]
    });
    let res =
        raw(port, "POST", "/api/strategies/mf-user/manifest?token=tok-ok", Some(&bad.to_string()))
            .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("9bad key"), "{res}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "校验失败不得改写清单");

    std::env::remove_var("RICOW_ROOT");
    let _ = std::fs::remove_dir_all(&root);
}

/// 递归列出目录下全部文件(测试小工具)。
fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else {
            out.push(p);
        }
    }
    out
}
