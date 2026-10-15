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

    let res = raw(port, "GET", "/api/strategies/shannon_grid/source?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""id":"shannon_grid""#), "{body}");
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
    let entry = catalog::find("shannon_grid").expect("内置清单 shannon_grid 必须存在");
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
    let res =
        raw(port, "POST", "/api/strategies/shannon_grid/ai-edit?token=tok-ok", Some(&post("   ")))
            .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("instruction"), "{res}");

    // ② 非 JSON → 400。
    let res =
        raw(port, "POST", "/api/strategies/shannon_grid/ai-edit?token=tok-ok", Some("{ not json"))
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
        "/api/strategies/shannon_grid/ai-edit?token=tok-ok",
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
    let res = raw(port, "GET", "/api/strategies/shannon_grid?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""id":"shannon_grid""#), "{body}");
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

/// 054 T35: 第 6 类 —— 会下单却毫无风控线索 → 提示; 但**只提示**, 且**不得误伤**。
#[test]
fn test_lint_lua_sixth_class_is_a_hint_never_a_block() {
    // 会下单 + 完全没有止损/仓位上限线索 → 提示(且必须点明"平台不代做风控")。
    let bare = "ctx:place_order({ pair = p, side = \"buy\", size = 1 })\n";
    let w = super::lint_lua(bare);
    assert!(w.iter().any(|m| m.contains("不代做盈亏风控")), "实际: {w:?}");

    // 有止损线索 → 不提示。
    let with_sl = "local stop_loss = 0.95\nctx:place_order({ pair = p })\n";
    assert!(
        super::lint_lua(with_sl).iter().all(|m| !m.contains("不代做盈亏风控")),
        "有止损线索就不该再提示: {:?}",
        super::lint_lua(with_sl)
    );
    // 只有仓位上限线索也算(不是必须有止损才算有风控)。
    let with_cap = "if pos > max_position then return end\nctx:place_order({ pair = p })\n";
    assert!(
        super::lint_lua(with_cap).iter().all(|m| !m.contains("不代做盈亏风控")),
        "有仓位上限就不该再提示: {:?}",
        super::lint_lua(with_cap)
    );
    // 纯观察型脚本(不下单)不该被提示 —— 对它谈"止损"是噪音。
    let watch = "ctx:log(\"price=\" .. ctx:price())\n";
    assert!(super::lint_lua(watch).is_empty(), "纯观察脚本不该有提示: {watch:?}");
}

/// 055 B4: 整份级第 6 类**不得被逐行警告挤掉**。
///
/// 之前 `lint_lua` 用一个 8 条的总上限同时管逐行与整份级, 而逐行类**每行都能触发**:
/// 十行 `config_f64(...) or ...` 就把配额吃满, 于是"这份脚本会下单但没有任何止损线索"
/// —— 对普通投资者最要紧的那句 —— 恰好就是看不见的那句。
#[test]
fn test_lint_lua_whole_file_warning_survives_a_flood_of_line_warnings() {
    // 12 行会触发 ② 类(`config_f64(...) or ...`), 足以把 8 条逐行配额吃满并溢出。
    let mut src = String::new();
    for i in 0..12 {
        src.push_str(&format!("  local v{i} = ctx:config_f64(\"k{i}\") or {i}\n"));
    }
    // 再加一行下单、且**没有任何**止损/仓位上限线索。
    src.push_str("ctx:place_order({ pair = p, side = \"buy\", size = 1 })\n");

    let w = super::lint_lua(&src);
    assert!(
        w.iter().any(|m| m.contains("不代做盈亏风控")),
        "逐行警告刷屏时, 整份级第 6 类必须还在: {w:?}"
    );
    // 而且要排在**第一条** —— 它是最该先看到的一句, 塞在末尾等于没说。
    assert!(
        w.first().is_some_and(|m| m.contains("不代做盈亏风控")),
        "整份级结论应排在最前(最要紧的一句): {w:?}"
    );
    // 逐行类仍受 8 条上限约束(防刷屏这条纪律本身没变)。
    let line_warns = w.iter().filter(|m| m.contains("行:")).count();
    assert!(line_warns <= super::LINE_WARN_CAP, "逐行类仍须 ≤ 8 条, 实际 {line_warns}: {w:?}");
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
        r#"{"name":"shannon_grid","market":"spot","pair":"ETHUSDT","idea":"随便"}"#.to_string(),
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
    //    054 P2 起「合法意图」必须带上本金与风险档位(两者缺一即 400)—— 这是审计 #2 的落点。
    let idea = "UNIQUE-IDEA-双均线金叉做多";
    let body = format!(
        r#"{{"name":"gen-x","market":"spot","pair":"ETHUSDT","interval":"1h","idea":"{idea}","initial_cash":10000,"risk_tolerance":"balanced"}}"#
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

/// 054 P1: `verify_days` 越界必须在 **AI 调用之前**就 400 —— 否则用户白白等一轮生成,
/// 才被告知窗口不合法(而且没配密钥时会先撞 403, 掩盖真正的问题)。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_ai_generate_verify_days_bounds_offline() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let saved = ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"]
        .map(std::env::var)
        .map(|r| r.ok());
    for k in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"] {
        std::env::remove_var(k);
    }

    let root = tmp_root("aigen-vdays");
    let port = boot(root.clone()).await;
    let post = |json: String| async move {
        raw(port, "POST", "/api/strategies/ai-generate?token=tok-ok", Some(&json)).await
    };
    // 054 P2 起本金与风险档位是必填 → 窗口用例也得带上, 否则卡在结构化输入那一步。
    let base = |days: &str| {
        format!(
            r#"{{"name":"gen-v","market":"spot","pair":"ETHUSDT","idea":"双均线","initial_cash":10000,"risk_tolerance":"balanced","verify_days":{days}}}"#
        )
    };

    // ① 0 天 / 超上界 → 400, 且原因是 verify_days(不是 403 need_keys)。
    for bad in ["0", "3651", "100000"] {
        let res = post(base(bad)).await;
        assert_eq!(status_of(&res), 400, "verify_days={bad} 应 400: {res}");
        let b = body_of(&res);
        assert!(b.contains("verify_days"), "400 要点名 verify_days: {b}");
        assert!(!b.contains("need_keys"), "窗口校验必须先于 AI 调用: {b}");
    }

    // ② 边界内(含 1 与 3650)不该因窗口被拒 —— 这里没配密钥, 应当走到 403(说明校验放行了)。
    for ok_days in ["1", "90", "3650"] {
        let res = post(base(ok_days)).await;
        assert_eq!(status_of(&res), 403, "verify_days={ok_days} 应通过校验: {res}");
    }

    // ③ 显式 verify=false 时窗口仍要合法(跳过验证 ≠ 可以传垃圾 ——
    //    否则"这个数到底有没有生效"会变成悬案)。
    let res = post(
        r#"{"name":"gen-v","market":"spot","pair":"ETHUSDT","idea":"双均线","initial_cash":10000,"risk_tolerance":"balanced","verify":false,"verify_days":99999}"#
            .to_string(),
    )
    .await;
    assert_eq!(status_of(&res), 400, "verify=false 也要校验显式给出的窗口: {res}");
    assert!(body_of(&res).contains("verify_days"), "{res}");

    for (k, v) in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].iter().zip(saved) {
        if let Some(val) = v {
            std::env::set_var(k, val);
        } else {
            std::env::remove_var(k);
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// 054 P2: 结构化投资输入的**纯函数**校验(T17)。
///
/// 这里断言的是"哪些输入必须被拒": 缺本金 / 档位不合法 / 非自定义档夹带自定义数字 /
/// 自定义档缺字段 / 现货带杠杆 / 百分数越界 / 杠杆越界。
/// 一句话概括取舍: **凡是会让人以为"我填的数生效了"的组合, 一律 400 而不是静默忽略**。
#[test]
fn test_resolve_risk_input_rejects_ambiguous_combinations() {
    let r =
        |market: &str,
         cash: Option<f64>,
         tier: Option<&str>,
         dd: Option<f64>,
         pos: Option<f64>,
         lev: Option<f64>,
         sp: Option<f64>| { resolve_risk_input(market, cash, tier, dd, pos, lev, sp) };
    // ① 本金必填且必须 > 0。
    for bad in [None, Some(0.0), Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
        let e = r("spot", bad, Some("balanced"), None, None, None, None).unwrap_err();
        assert!(e.contains("initial_cash"), "本金非法({bad:?})应点名 initial_cash: {e}");
    }
    // ② 档位必填且必须认得。
    let e = r("spot", Some(1000.0), None, None, None, None, None).unwrap_err();
    assert!(e.contains("risk_tolerance"), "{e}");
    let e = r("spot", Some(1000.0), Some("随便"), None, None, None, None).unwrap_err();
    assert!(e.contains("risk_tolerance") && e.contains("随便"), "{e}");
    // ③ 非自定义档夹带自定义数字 → 拒(否则"我填了 5% 却没生效")。
    let e = r("spot", Some(1000.0), Some("balanced"), Some(5.0), None, None, None).unwrap_err();
    assert!(e.contains("custom"), "非自定义档带自定义字段应被拒: {e}");
    // ④ 自定义档缺字段 → 拒; 现货不该要杠杆, 合约才要。
    let e = r("spot", Some(1000.0), Some("custom"), None, None, None, None).unwrap_err();
    assert!(e.contains("max_drawdown_pct"), "{e}");
    let e = r("spot", Some(1000.0), Some("custom"), Some(10.0), None, None, None).unwrap_err();
    assert!(e.contains("max_position_pct"), "{e}");
    let e =
        r("futures", Some(1000.0), Some("custom"), Some(10.0), Some(10.0), None, None).unwrap_err();
    assert!(e.contains("leverage_cap"), "合约自定义档必须给杠杆上限: {e}");
    // ⑤ 现货 + 杠杆 → 400(现货没有杠杆这回事, 收下就是虚假承诺)。
    let e = r("spot", Some(1000.0), Some("custom"), Some(10.0), Some(10.0), Some(3.0), None)
        .unwrap_err();
    assert!(e.contains("杠杆") || e.contains("leverage"), "现货带杠杆应被拒: {e}");
    // ⑥ 百分数与杠杆的范围。
    for bad in [0.0, -1.0, 100.5] {
        let e =
            r("spot", Some(1000.0), Some("custom"), Some(bad), Some(10.0), None, None).unwrap_err();
        assert!(e.contains("max_drawdown_pct"), "{bad} 应越界: {e}");
    }
    for bad in [0.5, 10.5] {
        let e = r("futures", Some(1000.0), Some("custom"), Some(10.0), Some(10.0), Some(bad), None)
            .unwrap_err();
        assert!(e.contains("leverage_cap"), "杠杆 {bad} 应越界: {e}");
    }
    // ⑦ 合法组合能过: 现货自定义档不给间距也行(间距只对网格类有意义)。
    let (p, cash) =
        r("spot", Some(1000.0), Some("custom"), Some(10.0), Some(10.0), None, None).unwrap();
    assert_eq!(cash, 1000.0);
    assert_eq!(p.tier.label_zh(), "自定义");
    // 预设档不该被用户的额外字段影响 —— 它本来就会被拒, 这里是正向确认档位数字来自表。
    let (p, _) = r("futures", Some(1000.0), Some("aggressive"), None, None, None, None).unwrap();
    assert_eq!(p.leverage_cap, Some(5.0));
    assert_eq!(p.max_position_pct, Some(50.0));
    // 现货下预设档的杠杆不生效(渲染时会改成"现货不使用杠杆")。
    let (p, _) = r("spot", Some(1000.0), Some("aggressive"), None, None, None, None).unwrap();
    assert_eq!(p.leverage_cap, Some(5.0), "档位表本身带值, 由渲染层按市场决定用不用");
}

/// 054 P2/T19: 结构化输入的校验必须发生在 **AI 调用之前**(没配密钥时应看到 400, 不是 403)。
///
/// 判据: 没配密钥时任何请求都会走到 403 need_keys —— 所以"应 400"的用例若回了 403,
/// 就说明校验被放到 AI 调用之后了, 用户会白等几十秒才被告知输入不合法。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_ai_generate_risk_input_validation_offline() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let saved = ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"]
        .map(std::env::var)
        .map(|r| r.ok());
    for k in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"] {
        std::env::remove_var(k);
    }

    let root = tmp_root("aigen-risk");
    let port = boot(root.clone()).await;
    let post = |json: String| async move {
        raw(port, "POST", "/api/strategies/ai-generate?token=tok-ok", Some(&json)).await
    };
    let base = r#"{"name":"gen-r","market":"spot","pair":"ETHUSDT","idea":"双均线"}"#;

    // 缺本金 / 缺档位 → 400(而不是 403)。
    for (label, body) in [
        ("缺本金", base.to_string()),
        (
            "缺档位",
            r#"{"name":"gen-r","market":"spot","pair":"ETHUSDT","idea":"x","initial_cash":1000}"#
                .to_string(),
        ),
        (
            "档位非法",
            r#"{"name":"gen-r","market":"spot","pair":"ETHUSDT","idea":"x","initial_cash":1000,"risk_tolerance":"随便"}"#
                .to_string(),
        ),
        (
            "非自定义档夹带数字",
            r#"{"name":"gen-r","market":"spot","pair":"ETHUSDT","idea":"x","initial_cash":1000,"risk_tolerance":"balanced","max_drawdown_pct":5}"#
                .to_string(),
        ),
    ] {
        let res = post(body).await;
        assert_eq!(status_of(&res), 400, "{label} 应 400: {res}");
    }

    // 合法输入 → 通过校验(没配密钥, 于是撞在 403 need_keys 上)。
    let res = post(
        r#"{"name":"gen-r","market":"spot","pair":"ETHUSDT","idea":"x","initial_cash":1000,"risk_tolerance":"conservative"}"#
            .to_string(),
    )
    .await;
    assert_eq!(status_of(&res), 403, "合法输入应通过校验(403 是没配密钥): {res}");
    assert!(body_of(&res).contains("need_keys"), "{res}");

    for (k, v) in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].iter().zip(saved) {
        if let Some(val) = v {
            std::env::set_var(k, val);
        } else {
            std::env::remove_var(k);
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// 054 D7: 来源标记的写入与判定(**幂等**, 且安全默认 = 用户策略)。
#[test]
fn test_origin_marker_is_idempotent_and_defaults_to_user() {
    assert!(!origin_is_ai("# nothing here\n[params]\n"), "没有标记 = 用户策略(安全默认)");
    assert!(!origin_is_ai(""), "空文件 = 用户策略");
    // 必须整行匹配: 正文里出现同形字符串不该被误判。
    assert!(!origin_is_ai("# see # ricow:origin = ai ?\nnote = \"# ricow:origin = ai\""));
    assert!(origin_is_ai(&format!("{ORIGIN_AI_MARKER}\n[params]\n")));

    let dir = std::env::temp_dir().join(format!("ricow-origin-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("probe.toml");
    std::fs::write(&path, "[params]\n").unwrap();
    mark_origin_ai(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(ORIGIN_AI_MARKER), "标记应在文件顶部: {text}");
    // 幂等: 再写一次不该出现两行。
    mark_origin_ai(&path).unwrap();
    let again = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, again, "重复标记不应叠加: {again}");
    let _ = std::fs::remove_dir_all(&dir);
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
        // N2 (2026-10-11): "空 name / 空 desc 被拒" 这两条用例**已按新契约移除** —— 留空不再非法,
        // 见本函数末尾的正向断言。之所以留这句注释: 谁把它们加回来, 谁就重新堵死了
        // "从脚本识别参数 → 保存清单" 这条路(N3 的 AI 调参数也因此根本进不去)。
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

    // N2 (2026-10-11): 中文名 / 说明 / 策略名**留空不再非法** —— 用户是脚本的使用者不是作者,
    // 从脚本识别出来的参数本就没有名字与说明。这三条必须是**正向**断言: 一旦有人把校验加回去,
    // "识别参数 → 一键保存" 会再次 400, 而 N3 的 AI 调参数功能会跟着一起进不去。
    let blank = build_manifest(
        "my-grid",
        "spot",
        ok(serde_json::json!([
            {"key": "spacing_pct", "name": "", "type": "f64", "desc": ""},
            {"key": "flag", "name": "   ", "type": "i64", "desc": "  "}
        ])),
    )
    .expect("留空的中文名与说明必须能保存(N2)");
    assert_eq!(blank.params.len(), 2);
    assert!(blank.params[0].name.is_empty(), "空中文名应原样保留为空, 不得被填上假数据");
    assert!(blank.params[0].desc.is_empty());

    let mut blank_name = ok(serde_json::json!([]));
    blank_name.name = "   ".into();
    let m_blank =
        build_manifest("my-grid", "spot", blank_name).expect("策略中文名留空必须能保存(N2)");
    assert!(m_blank.name.is_empty());
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

// ---- 052 方案 1: 复制带清单 ----

/// `derive_manifest` 只改 id / market, 其余(含参数 schema 与展示元数据)逐字照搬。
#[test]
fn test_derive_manifest_rewrites_only_id_and_market() {
    let src = catalog::find("paired_grid").expect("内置 paired_grid 应在目录里");
    let d = derive_manifest(&src.manifest, "paired_grid-1", "spot");
    assert_eq!(d.id, "paired_grid-1", "id 必须改成新策略名(落盘文件名按它)");
    assert_eq!(d.market, "spot");
    assert_eq!(d.name, src.manifest.name, "中文名照搬(副本与源是同一个策略)");
    assert_eq!(d.summary, src.manifest.summary);
    assert_eq!(d.params.len(), src.manifest.params.len(), "参数 schema 必须整份带过来");
    let keys: Vec<&str> = d.params.iter().map(|p| p.key.as_str()).collect();
    assert!(keys.contains(&"pair"), "保留键 pair 也照搬(表单侧自会跳过)");
    assert!(keys.contains(&"spacing_pct"));
    // 跨市场复制: market 必须改写, 否则 catalog 的 check_dir 会整条跳过。
    let d2 = derive_manifest(&src.manifest, "paired_grid-f", "futures");
    assert_eq!(d2.market, "futures");
    assert!(d2.check_dir("futures").is_ok());
}

/// `copy_from` 端到端: 复制内置 → 清单随行落盘 → 详情页 `declared=true` 且参数非空。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_copy_from_writes_manifest_and_lights_up_params() {
    // catalog 扫描读进程级 RICOW_ROOT, 与 manifest 用例同口径串行化。
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let root = tmp_root("copyfrom");
    std::env::set_var("RICOW_ROOT", &root);
    let port = boot(root.clone()).await;

    let src = catalog::find("paired_grid").expect("内置源");
    let n_params = src.manifest.params.len();
    let code = src.code.clone();

    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(
            &serde_json::json!({
                "name": "paired_grid-1", "market": "spot", "pair": "BTCUSDT",
                "code": code, "params": {}, "overwrite": false, "copy_from": "paired_grid"
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains("manifest_path"), "响应应回报清单落盘路径: {body}");

    // 清单落在 strategies/spot/paired_grid-1.toml(与内置清单同目录同构)。
    let mf = root.join("strategies").join("spot").join("paired_grid-1.toml");
    assert!(mf.is_file(), "清单须随复制落盘: {}", mf.display());
    let text = std::fs::read_to_string(&mf).unwrap();
    assert!(text.contains(r#"id = "paired_grid-1""#), "清单 id 必须改写: {text}");
    assert!(text.contains("spacing_pct"), "{text}");

    // 目录里这一条现在是"已声明"的, 参数表单据此渲染。
    let entry = catalog::find("paired_grid-1").expect("复制后应能扫到");
    assert!(entry.declared, "复制后必须是 declared=true(参数表单才有内容)");
    assert_eq!(entry.manifest.params.len(), n_params, "参数 schema 应与源一致");

    // 详情端点同样点亮(前端参数表单的数据源)。
    let res = raw(port, "GET", "/api/strategies/paired_grid-1?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains(r#""declared":true"#), "{body}");
    assert!(body.contains(r#""key":"spacing_pct""#), "{body}");
    assert!(body.contains("间距百分比"), "参数中文名应随清单带过来: {body}");

    // 不带 copy_from 的普通新建: 清单不落盘, 仍是未声明(既有行为不变)。
    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(
            &serde_json::json!({
                "name": "plain-new", "market": "spot", "pair": "BTCUSDT", "code": code
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");
    assert!(!body_of(&res).contains("manifest_path"), "非复制路径不应报清单路径: {res}");
    assert!(!root.join("strategies").join("spot").join("plain-new.toml").exists());

    std::env::remove_var("RICOW_ROOT");
    let _ = std::fs::remove_dir_all(&root);
}

/// `copy_from` 的两条拒绝: 未知源 / 与 overwrite 互斥 —— 都必须在**写盘之前** 400。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_copy_from_rejects_unknown_source_and_overwrite_combo() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let root = tmp_root("copyfrombad");
    std::env::set_var("RICOW_ROOT", &root);
    let port = boot(root.clone()).await;
    let code = "function on_tick(ctx)\n    return {}\nend\n";
    let post = |body: serde_json::Value| {
        let body = body.to_string();
        async move { raw(port, "POST", "/api/strategies?token=tok-ok", Some(&body)).await }
    };

    // 未知源 → 400 中文, 且**什么都没落盘**。
    let res = post(serde_json::json!({
        "name": "c1", "market": "spot", "pair": "BTCUSDT", "code": code, "copy_from": "no-such-src"
    }))
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("no-such-src"), "{res}");
    assert!(!root.join("strategies").join("c1.toml").exists(), "失败不得落盘");
    assert!(!root.join("strategies").join("spot").join("c1.lua").exists(), "失败不得落盘");

    // copy_from + overwrite=true → 400(复制是新建语义)。
    let res = post(serde_json::json!({
        "name": "c2", "market": "spot", "pair": "BTCUSDT", "code": code,
        "copy_from": "paired_grid", "overwrite": true
    }))
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("互斥"), "{res}");
    assert!(!root.join("strategies").join("spot").join("c2.toml").exists(), "失败不得落盘");

    std::env::remove_var("RICOW_ROOT");
    let _ = std::fs::remove_dir_all(&root);
}

/// 复制一个**自己没声明清单**的用户策略 → 400(复制带不来说明, 该走「从脚本识别参数」)。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_copy_from_rejects_undeclared_source() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let root = tmp_root("copyfrombare");
    std::env::set_var("RICOW_ROOT", &root);
    let port = boot(root.clone()).await;
    let code =
        "function on_tick(ctx)\n    local p = ctx:config_str(\"pair\")\n    return {}\nend\n";

    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(
            &serde_json::json!({"name": "bare-src", "market": "spot", "pair": "ETHUSDT", "code": code})
                .to_string(),
        ),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");

    let res = raw(
        port,
        "POST",
        "/api/strategies?token=tok-ok",
        Some(
            &serde_json::json!({
                "name": "bare-copy", "market": "spot", "pair": "ETHUSDT",
                "code": code, "copy_from": "bare-src"
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("没有声明参数清单"), "{res}");
    assert!(body_of(&res).contains("从脚本识别参数"), "要给出下一步: {res}");

    std::env::remove_var("RICOW_ROOT");
    let _ = std::fs::remove_dir_all(&root);
}

// ---- 052 方案 2: 无状态参数提取端点 ----

#[test]
fn test_parse_ai_mode() {
    assert_eq!(parse_ai_mode(None).unwrap(), AiEditMode::Code, "缺省向后兼容旧前端");
    assert_eq!(parse_ai_mode(Some("")).unwrap(), AiEditMode::Code);
    assert_eq!(parse_ai_mode(Some("code")).unwrap(), AiEditMode::Code);
    assert_eq!(parse_ai_mode(Some(" PARAMS ")).unwrap(), AiEditMode::Params);
    let err = parse_ai_mode(Some("magic")).unwrap_err();
    assert!(err.contains("code|params") && err.contains("magic"), "{err}");
}

/// 无状态端点: 喂一段内置 Lua, 抽出的键集合与清单声明一致; 空代码 400。
#[tokio::test]
async fn test_extract_params_endpoint() {
    let root = tmp_root("extract");
    let port = boot(root).await;

    let lua = include_str!("../../../../../strategies/spot/paired_grid.lua");
    let res = raw(
        port,
        "POST",
        "/api/strategies/extract-params?token=tok-ok",
        Some(&serde_json::json!({"code": lua}).to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    for key in ["spacing_pct", "accumulate_mode", "pair", "interval", "min_pair_profit"] {
        assert!(body.contains(&format!(r#""key":"{key}""#)), "缺 {key}: {body}");
    }
    // 类型取 API 语义: `num` → f64, `cfg_str` → string。
    assert!(body.contains(r#""key":"spacing_pct","type":"f64""#), "{body}");

    // 空代码 → 400(没代码就无从识别); 无 token → 401(token 门禁之后)。
    let res = raw(
        port,
        "POST",
        "/api/strategies/extract-params?token=tok-ok",
        Some(&serde_json::json!({"code": "   "}).to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    let res = raw(
        port,
        "POST",
        "/api/strategies/extract-params",
        Some(&serde_json::json!({"code": lua}).to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 401, "{res}");
    assert!(body_of(&res).is_empty(), "401 须空体: {res}");
}

// ---- N1: 删除用户策略 ----

/// N1: 删除的**候选位置**要覆盖全部真实布局, 且**不能**误伤名字相近的别的策略。
///
/// 两条判据缺一不可:
/// 1. **覆盖** —— 031 FR-006 起脚本落在 `strategies/{market}/`, 只认根目录就删不掉脚本
///    (策略"删了"却仍在列表里)。这里连旧布局的根 `.lua` 与两个市场目录一起圈。
/// 2. **精确** —— `.bak` 前缀判定必须是 `{id}.`(**带点**): 少了那个点, 删 `grid` 会顺手删掉
///    `grid_futures.lua.<ts>.bak`, 而备份是用户唯一的回退手段。
#[test]
fn test_strategy_delete_candidates_cover_layout_and_scoped_to_id() {
    let root = std::env::temp_dir().join(format!("ricow-del-targets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("strategies");
    let spot = dir.join("spot");
    std::fs::create_dir_all(&spot).expect("建目录");
    // 本策略: 实例 TOML + 市场目录脚本 + 清单 + 两处备份。
    for f in [
        "grid.toml",
        "spot/grid.lua",
        "spot/grid.toml",
        "grid.toml.20261012T001122.bak",
        "spot/grid.lua.20261012T001122.bak",
    ] {
        std::fs::write(dir.join(f), "x").expect("写文件");
    }
    // 同前缀的**另一个**策略: 一个文件都不能被选中。
    for f in
        ["grid_futures.toml", "spot/grid_futures.lua", "spot/grid_futures.lua.20261012T001122.bak"]
    {
        std::fs::write(dir.join(f), "x").expect("写文件");
    }

    let mut got: Vec<String> = crate::commands::strategy_file_candidates(&root, "grid")
        .into_iter()
        .filter(|p| p.exists())
        .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            "strategies/grid.toml",
            "strategies/grid.toml.20261012T001122.bak",
            "strategies/spot/grid.lua",
            "strategies/spot/grid.lua.20261012T001122.bak",
            "strategies/spot/grid.toml",
        ],
        "应只圈定本策略的全部真实文件"
    );

    // 同一份判据喂给删除函数: **市场目录里的脚本必须被删掉** —— 这正是 031 之后的漏删点
    // (旧实现只认 `strategies/{name}.lua`, 脚本删不掉, 策略仍会被 catalog 扫到)。
    let removed = crate::commands::delete_strategy_files_in(&root, "grid").expect("应删成功");
    assert!(removed.iter().any(|f| f == "grid.lua"), "市场目录脚本应被删: {removed:?}");
    assert!(!spot.join("grid.lua").exists(), "市场目录脚本必须真的没了");
    assert!(spot.join("grid_futures.lua").is_file(), "同前缀的别的策略不能被动到");
    let _ = std::fs::remove_dir_all(&root);
}

/// N1 端点: 删自定义策略 → 三件套 + 备份都没了; 内置 409 reserved; 未知/非法名 404; 无 token 401。
#[tokio::test]
async fn test_delete_strategy_endpoint() {
    let root = tmp_root("del");
    let port = boot(root.clone()).await;

    // 先落一个自定义策略(走真实保存端点)。
    let body = serde_json::json!({
        "name": "del_probe", "market": "spot", "pair": "ETHUSDT",
        "params": {}, "code": "function on_tick(ctx) end",
        "overwrite": false
    })
    .to_string();
    let res = raw(port, "POST", "/api/strategies?token=tok-ok", Some(&body)).await;
    assert_eq!(status_of(&res), 200, "前置: 保存应成功: {res}");

    let dir = root.join("strategies");
    assert!(dir.join("del_probe.toml").is_file(), "前置: 实例 TOML 应在");
    assert!(dir.join("spot").join("del_probe.lua").is_file(), "前置: 脚本应在");
    // 造一个同名备份(验证它也进删除清单), 与一个**同前缀**的邻居(验证它不受连累)。
    std::fs::write(dir.join("del_probe.toml.20261012T001122.bak"), "old").expect("写备份");
    std::fs::write(dir.join("spot").join("del_probe2.lua"), "x").expect("写邻居");

    // 无 token → 401 空体(与其它策略端点同门禁)。
    let res = raw(port, "DELETE", "/api/strategies/del_probe", None).await;
    assert_eq!(status_of(&res), 401, "{res}");
    assert!(body_of(&res).is_empty(), "401 须空体: {res}");

    // 内置 → 409 reserved(与保存同口径, 单一来源 catalog::is_builtin_id)。
    let res = raw(port, "DELETE", "/api/strategies/paired_grid?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 409, "{res}");
    assert!(body_of(&res).contains("reserved"), "{res}");

    // 未知 → 404。
    let res = raw(port, "DELETE", "/api/strategies/no_such_thing?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 404, "{res}");

    // 非法名(带路径分隔) → 404, 不穿越也不回显。
    let res = raw(port, "DELETE", "/api/strategies/..%2Fetc?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 404, "{res}");

    // 正常删除 → 200, 回包**如实列出**删了哪些。
    let res = raw(port, "DELETE", "/api/strategies/del_probe?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 200, "{res}");
    let body = body_of(&res);
    assert!(body.contains("del_probe.toml"), "回包应列出删掉的文件: {res}");
    assert!(body.contains(".bak"), "备份也应被删掉: {res}");
    assert!(!dir.join("del_probe.toml").exists(), "实例 TOML 应已删");
    assert!(!dir.join("spot").join("del_probe.lua").exists(), "脚本应已删");
    assert!(!dir.join("del_probe.toml.20261012T001122.bak").exists(), "备份应已删");
    assert!(dir.join("spot").join("del_probe2.lua").is_file(), "同前缀的别的策略不能被动到");

    // 再删一次 → 404(已经不是策略了)。
    let res = raw(port, "DELETE", "/api/strategies/del_probe?token=tok-ok", None).await;
    assert_eq!(status_of(&res), 404, "{res}");
}

// ---- N2-C: AI 给参数起中文名(纯函数部分) ----

/// N2-C: 命名结果**只保留静态提取到的键** —— 模型多给的键丢弃, 少给的就是没命名。
///
/// 与 `keep_manifest_params` 同一纪律(052 的键过滤), 但过滤依据不同: 这里的"合法键集合"
/// 来自**代码扫描**而不是清单 —— 因为需要起名的场景恰恰是"清单还没建起来"的时候。
#[test]
fn test_parse_names_filters_to_extracted_keys() {
    let hints = crate::strategies::params::extract_param_hints(
        "local a = num(ctx, \"spacing_pct\", 0.01)\n\
         local b = ctx:config_str(\"pair\")\n",
    );
    assert_eq!(hints.len(), 2, "前置: 应先扫出两个键");

    // 模型: 多给了一个代码里没有的键; 两个真键里一个给了 name+desc, 一个只给字符串。
    let raw = "{\"spacing_pct\": {\"name\": \"间距百分比\", \"desc\": \"上下挂单的间距\"},\n\
               \"编造的键\": {\"name\": \"不该出现\"},\n\
               \"pair\": \"交易对\"}";
    // `WebError` 没派生 Debug, 所以用 unwrap_or_else 而不是 expect。
    let got = parse_names(raw, &hints).unwrap_or_else(|_| panic!("应解析成功"));
    let keys: Vec<&str> = got.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(keys, vec!["spacing_pct", "pair"], "顺序随静态提取; 编造的键必须被丢掉");
    assert_eq!(got[0].name, "间距百分比");
    assert_eq!(got[0].desc.as_deref(), Some("上下挂单的间距"));
    assert_eq!(got[1].name, "交易对");
    assert!(got[1].desc.is_none(), "只给了名字时说明留空(None), 不是空串");
}

/// N2-C: 一个都没命名成 → 报错, 而不是回一个空列表假装成功。
#[test]
fn test_parse_names_rejects_empty_and_bad_json() {
    let hints = crate::strategies::params::extract_param_hints(
        "local a = num(ctx, \"spacing_pct\", 0.01)\n",
    );
    // 模型回的键全都不在代码里。
    assert!(parse_names("{\"other\": {\"name\": \"x\"}}", &hints).is_err());
    // 名字是空串 → 不算命名。
    assert!(parse_names("{\"spacing_pct\": {\"name\": \"  \"}}", &hints).is_err());
    // 根本不是 JSON。
    assert!(parse_names("我不会起名", &hints).is_err());
}

// ---- 054 T37: 生成后补摘要 + 参数中文名(纯函数部分) ----

/// T37: 摘要是**一行**: 模型给了两段就只取第一段, 空的就是没有(不拿别的东西顶上)。
#[test]
fn test_clamp_summary_takes_one_line_and_caps_length() {
    assert_eq!(clamp_summary("  "), None, "空摘要就是没有摘要");
    assert_eq!(clamp_summary("高抛低吸的网格").as_deref(), Some("高抛低吸的网格"));
    // 换行后面那句是模型的多余解释, 不该进清单。
    assert_eq!(clamp_summary("网格策略\n\n- 涨了卖\n- 跌了买").as_deref(), Some("网格策略"));

    let long: String = "涨".repeat(200);
    let got = clamp_summary(&long).unwrap_or_else(|| panic!("超长也该有值"));
    assert!(got.ends_with('…'), "超长必须截断并留省略号: {got}");
    assert_eq!(got.chars().count(), SUMMARY_MAX_CHARS + 1, "80 字 + 省略号: {got}");
}

/// T37: `_summary` 要被读成摘要, 且**不能**顺带变成一行参数。
///
/// 这是这次最容易出的错: 命名解析的纪律是"模型多给的键一律丢弃", 而 `_summary` 在 JSON 里
/// 恰恰长得像一个多给的键 —— 不先摘出来就会静默变成一个假参数。
#[test]
fn test_parse_generated_meta_reads_summary_without_leaking_it_as_a_param() {
    let hints = crate::strategies::params::extract_param_hints(
        "local a = num(ctx, \"spacing_pct\", 0.01)\n",
    );
    let raw = "{\"_summary\": \"按固定间距高抛低吸\", \
               \"spacing_pct\": {\"name\": \"间距百分比\", \"desc\": \"上下挂单的间距\"}}";
    let (summary, names) =
        parse_generated_meta(raw, &hints).unwrap_or_else(|_| panic!("应解析成功"));
    assert_eq!(summary.as_deref(), Some("按固定间距高抛低吸"));
    assert_eq!(names.len(), 1, "_summary 不是参数: {names:?}");
    assert_eq!(names[0].key, "spacing_pct");

    // 模型压根没给 _summary → 摘要 None, 但参数命名照旧生效(两个能力互不牵连)。
    let (no_summary, names2) =
        parse_generated_meta("{\"spacing_pct\": {\"name\": \"间距百分比\"}}", &hints)
            .unwrap_or_else(|_| panic!("应解析成功"));
    assert!(no_summary.is_none());
    assert_eq!(names2.len(), 1);
}

/// T37: 合并成清单参数时, **类型与默认值只认静态提取**(模型说的不算),
/// 说明则**注释优先于 AI** —— 作者自己写的那句比模型照着代码现编的更可信。
#[test]
fn test_merge_manifest_params_keeps_code_facts_and_prefers_comment_desc() {
    let hints = crate::strategies::params::extract_param_hints(
        // 注释说明的认读形态是「-- 键 空格 说明」(与内置清单块同形), 键后不能带冒号。
        "--   spacing_pct     作者自己写的间距说明\n\
         local a = num(ctx, \"spacing_pct\", 0.01)\n\
         local b = ctx:config_i64(\"levels\", 5)\n",
    );
    assert_eq!(hints.len(), 2, "前置: 两个键, 类型分别是 f64 / i64");
    let named = vec![
        NamedParam {
            key: "spacing_pct".to_string(),
            name: "间距百分比".to_string(),
            desc: Some("模型现编的说明".to_string()),
        },
        NamedParam { key: "levels".to_string(), name: "档位数".to_string(), desc: None },
        NamedParam {
            key: "代码里没有的键".to_string(), name: "不该出现".to_string(), desc: None
        },
    ];
    let got = merge_manifest_params(hints, &named);
    assert_eq!(got.len(), 2, "只按静态提取出参: {got:?}");
    // 类型/默认值来自代码: 模型没插手的余地。
    assert!(matches!(got[0].ty, ParamType::Float));
    assert!(matches!(got[0].default, Some(ConfigValue::Float(f)) if (f - 0.01).abs() < 1e-9));
    assert!(matches!(got[1].ty, ParamType::Int));
    assert!(matches!(got[1].default, Some(ConfigValue::Integer(5))));
    // 名字来自 AI。
    assert_eq!(got[0].name, "间距百分比");
    assert_eq!(got[1].name, "档位数");
    // 说明: 注释那句赢了 AI 那句; 没有注释也没有 AI 说明时留空(不是编一句)。
    assert_eq!(got[0].desc, "作者自己写的间距说明");
    assert_eq!(got[1].desc, "");
    // 合并结果必须能直接过 `build_manifest` 的校验(键合法、不重复)。
    let req = ManifestRequest {
        name: String::new(),
        summary: "s".to_string(),
        description: String::new(),
        suitable: String::new(),
        unsuitable: String::new(),
        params: got,
    };
    assert!(build_manifest("x", "spot", req).is_ok(), "合并结果应可直接落清单");
}

// ---- 052 方案 3: AI 调参数(纯函数部分) ----

#[test]
fn test_extract_json_object_strips_fences_and_noise() {
    assert_eq!(extract_json_object(r#"{"a":1}"#).as_deref(), Some(r#"{"a":1}"#));
    assert_eq!(
        extract_json_object("```json\n{\"a\": 1}\n```").as_deref(),
        Some(r#"{"a": 1}"#),
        "围栏要剥掉"
    );
    assert_eq!(
        extract_json_object("好的:\n{\"a\":1}\n以上").as_deref(),
        Some(r#"{"a":1}"#),
        "前后寒暄要切掉"
    );
    assert_eq!(extract_json_object("没有任何对象"), None);
    assert_eq!(extract_json_object("[1,2]"), None, "数组不是对象");
}

#[test]
fn test_current_values_text_is_sorted_and_readable() {
    // 无实例 → 一句可读的说明(不是空串, 免得提示词里出现空段)。
    assert!(current_values_text(None).contains("还没有保存过"));
    let v = InstanceValues {
        pair: Some("BTCUSDT".into()),
        current: HashMap::from([
            ("spacing_pct".to_string(), ConfigValue::Float(0.02)),
            ("accumulate_mode".to_string(), ConfigValue::String("u".into())),
            ("atr_period".to_string(), ConfigValue::Integer(14)),
        ]),
    };
    let text = current_values_text(Some(&v));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "pair + 3 个参数: {text}");
    assert_eq!(lines[0], "- accumulate_mode = \"u\"", "{text}");
    assert!(text.contains("- atr_period = 14"), "{text}");
    assert!(text.contains("- pair = \"BTCUSDT\""), "pair 也要给模型看到: {text}");
}

/// 键过滤: 清单外的键丢弃 + warning; 类型不符丢弃 + warning; 枚举取值必须合法。
#[test]
fn test_keep_manifest_params_filters_and_warns() {
    let manifest = catalog::find("paired_grid").unwrap().manifest;
    let suggested = HashMap::from([
        ("spacing_pct".to_string(), ConfigValue::Float(0.02)),
        ("atr_period".to_string(), ConfigValue::Integer(14)), // 清单里没有 → 丢
        ("order_amount".to_string(), ConfigValue::Integer(20)), // f64 参数收整数 → 归一成浮点
        ("accumulate_mode".to_string(), ConfigValue::String("coin".into())),
        ("direction_offset".to_string(), ConfigValue::String("x".into())), // 类型不符 → 丢
    ]);
    let (kept, warns) = keep_manifest_params(suggested, &manifest);
    assert_eq!(kept.len(), 3, "{kept:?}");
    assert!(
        matches!(kept.get("order_amount"), Some(ConfigValue::Float(f)) if (*f - 20.0).abs() < 1e-9)
    );
    assert!(matches!(kept.get("accumulate_mode"), Some(ConfigValue::String(s)) if s == "coin"));
    assert!(!kept.contains_key("atr_period"), "清单外的键绝不能写进 TOML");
    assert!(!kept.contains_key("direction_offset"), "类型不符必须丢");

    let joined = warns.join("\n");
    assert!(joined.contains("atr_period") && joined.contains("不在本策略清单里"), "{joined}");
    assert!(joined.contains("改代码逻辑"), "要告诉用户正确路径: {joined}");
    assert!(joined.contains("direction_offset") && joined.contains("需要数值"), "{joined}");

    // 枚举取值不在可选集里 → 丢并点名可选值。
    let (kept, warns) = keep_manifest_params(
        HashMap::from([("accumulate_mode".to_string(), ConfigValue::String("wat".into()))]),
        &manifest,
    );
    assert!(kept.is_empty());
    assert!(warns[0].contains("wat") && warns[0].contains("u/coin"), "{warns:?}");
}

// ---- 052 方案 3: AI 调参数的离线端点行为 ----

/// 未配密钥时两种模式都要 403 `need_keys`(params 模式也必须先在联网前拦住);
/// `mode` 非法与清单为空是同步 400, 不该走到网络。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_ai_edit_params_offline_guards() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    // 与 test_ai_edit_validation_and_need_keys_offline 同款: 清掉 AI 覆盖变量,
    // 确保走到"远程端点缺密钥"分支而不是真的去联网。
    let saved =
        ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].map(|k| std::env::var(k).ok());
    for k in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"] {
        std::env::remove_var(k);
    }

    let root = tmp_root("aiparams");
    let port = boot(root).await;

    // mode 非法 → 400(同步校验, 不联网, 不查策略)。
    let res = raw(
        port,
        "POST",
        "/api/strategies/paired_grid/ai-edit?token=tok-ok",
        Some(&serde_json::json!({"instruction": "间距调大点", "mode": "magic"}).to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("code|params"), "{res}");

    // params 模式 + 内置(有清单)但没配密钥 → 403 need_keys(证明它真的走到了 provider 的密钥前置)。
    let res = raw(
        port,
        "POST",
        "/api/strategies/paired_grid/ai-edit?token=tok-ok",
        Some(&serde_json::json!({"instruction": "把间距改成 0.02", "mode": "params"}).to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 403, "{res}");
    assert!(body_of(&res).contains("need_keys"), "{res}");

    // 未知 id → 404(与 source 同口径)。
    let res = raw(
        port,
        "POST",
        "/api/strategies/no-such-x/ai-edit?token=tok-ok",
        Some(&serde_json::json!({"instruction": "x", "mode": "params"}).to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 404, "{res}");

    for (k, v) in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].iter().zip(saved) {
        if let Some(val) = v {
            std::env::set_var(k, val);
        } else {
            std::env::remove_var(k);
        }
    }
}

// ---- N3-A: 多轮对话历史 + 改动对照(纯函数) ----

/// 历史截断只保留**最近**的部分, 并如实标出"更早的已省略"。
///
/// 判据刻意不看总长度是否变小, 而看**尾部是否完整保留** —— 跟进话("再保守点")依赖的就是尾部。
#[test]
fn test_cap_history_keeps_tail_and_marks_truncation() {
    assert_eq!(cap_history(None), "");
    assert_eq!(cap_history(Some("   \n ")), "", "全空白等于没历史");
    assert_eq!(cap_history(Some("  第一轮  ")), "第一轮", "两头空白要去掉");

    // 刚过上限: 保留尾部, 并以标记开头(shorter than 原文)。
    let long = format!("开头{}结尾", "x".repeat(MAX_HISTORY_CHARS + 100));
    let capped = cap_history(Some(&long));
    assert!(capped.starts_with("(更早的对话已省略)"), "{capped}");
    assert!(capped.ends_with("结尾"), "尾部必须完整保留: {capped}");
    assert_eq!(capped.chars().count(), MAX_HISTORY_CHARS + "(更早的对话已省略)\n".chars().count());

    // 正好等于上限: 原样(不加标记)。
    let exact = "y".repeat(MAX_HISTORY_CHARS);
    assert_eq!(cap_history(Some(&exact)), exact);
}

/// 回包切段: 说明里的花括号**不能**把 JSON 段切歪; 没有 JSON 时说明也要留下。
#[test]
fn test_split_advice_and_json_separates_narrative_from_object() {
    // 常规: 说明 + 末行 JSON。
    let (advice, json) =
        split_advice_and_json("整体调保守。\n\n- 间距放宽: 成交会变少\n\n{\"spacing_pct\": 0.02}");
    assert!(advice.contains("间距放宽") && !advice.contains('{'), "说明段应不含 JSON: {advice}");
    assert_eq!(json.as_deref(), Some("{\"spacing_pct\": 0.02}"));

    // 说明里出现花括号(集合写法)时, 仍认末行那个真 JSON。
    let (advice, json) =
        split_advice_and_json("参数集合 {间距, 资金} 一起动。\n{\"spacing_pct\": 0.02}");
    assert!(advice.contains("参数集合"), "{advice}");
    assert_eq!(json.as_deref(), Some("{\"spacing_pct\": 0.02}"));

    // 模型不遵守"不要围栏"也得认。
    let (advice, json) = split_advice_and_json("改好了。\n```json\n{\"order_amount\": 5}\n```");
    assert_eq!(advice, "改好了。");
    assert_eq!(json.as_deref(), Some("{\"order_amount\": 5}"));

    // 多行 JSON(缩进)同样要能切出来。
    let (advice, json) = split_advice_and_json("说明。\n{\n  \"a\": 1,\n  \"b\": 2\n}");
    assert_eq!(advice, "说明。");
    assert!(json.as_deref().unwrap().contains("\"b\": 2"), "{json:?}");

    // 一个花括号都没有 → 全算说明, JSON 为 None(调用方据此回中文 400, 但说明不丢)。
    let (advice, json) = split_advice_and_json("这组参数不该动: 你已经很保守了。");
    assert_eq!(advice, "这组参数不该动: 你已经很保守了。");
    assert!(json.is_none());
}

/// 改动对照: 只列真变了的键; `before` 的两种来源必须**分开标注**(那是运行事实, 不能混)。
#[test]
fn test_diff_params_reports_only_real_changes_with_source() {
    let manifest = catalog::find("paired_grid").unwrap().manifest;
    let kept = HashMap::from([
        ("spacing_pct".to_string(), ConfigValue::Float(0.02)), // 变了
        ("order_amount".to_string(), ConfigValue::Float(10.0)), // 与默认值相同 → 不算改动
    ]);

    // ① 从没保存过实例值: before 取清单默认值, 并标明"只是默认值"。
    let changed = diff_params(&kept, None, &manifest);
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].key, "spacing_pct");
    assert_eq!(changed[0].name, "间距百分比", "给人看的行要用中文名");
    assert_eq!(changed[0].before.as_deref(), Some("0.01"));
    assert_eq!(changed[0].after, "0.02");
    assert!(changed[0].before_is_default, "没保存过时必须标明改前值只是默认值");

    // ② 实例里保存过 0.01: before 就是**用户在跑的值**, 不再标默认。
    let values = InstanceValues {
        pair: Some("BTCUSDT".to_string()),
        current: HashMap::from([
            ("spacing_pct".to_string(), ConfigValue::Float(0.01)),
            ("order_amount".to_string(), ConfigValue::Float(10.0)),
        ]),
    };
    let changed = diff_params(&kept, Some(&values), &manifest);
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert!(!changed[0].before_is_default, "保存过的值不是默认值");

    // ③ 建议值与**当前值**相同 → 一个键都不进对照表(整组值仍会回给表单)。
    let same = HashMap::from([("spacing_pct".to_string(), ConfigValue::Float(0.01))]);
    assert!(diff_params(&same, Some(&values), &manifest).is_empty());

    // ④ 清单里既没默认值、实例里也没存过的键(必填项) → before 为空(不编一个"0")。
    let fresh = HashMap::from([("start_price".to_string(), ConfigValue::Float(123.0))]);
    let changed = diff_params(&fresh, Some(&values), &manifest);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].before, None, "没有依据就不要写改前值");
    assert_eq!(changed[0].after, "123");
}

// ---- N3-B: 评估提示词的输入排布(纯函数) ----

/// 指标清单: 只讲白名单里有值的; `null` 必须**跳过**(写成"无"会被模型当成 0)。
#[test]
fn test_metrics_brief_skips_missing_and_null() {
    let m = serde_json::json!({
        "pair": "BTCUSDT", "interval": "1h", "days": 90,
        "equity_change_pct": 12.5, "max_drawdown_pct": 18.23456,
        "sharpe": null, "annual_volatility_pct": 30.0,
        "total_trades": 42, "unknown_field": 999,
    });
    let brief = metrics_brief(&m).unwrap();
    assert!(brief.contains("窗口: BTCUSDT · 1h · 90 天"), "{brief}");
    assert!(brief.contains("- 区间收益 %: 12.5"), "{brief}");
    assert!(brief.contains("- 最大回撤 %: 18.2346"), "保留有效位, 只砍尾零: {brief}");
    assert!(brief.contains("- 年化波动率 %: 30"), "{brief}");
    assert!(brief.contains("- 成交笔数: 42"), "{brief}");
    assert!(!brief.contains("夏普"), "null 指标不许出现在提示词里: {brief}");
    assert!(
        !brief.contains("unknown_field") && !brief.contains("999"),
        "白名单外的字段不进: {brief}"
    );

    // 不是对象 / 一个白名单字段都没有 → 中文原因(由端点转 400)。
    assert!(metrics_brief(&serde_json::json!([1, 2])).is_err());
    assert!(metrics_brief(&serde_json::json!({"pair": "BTCUSDT"})).is_err());
}

/// 改动清单: 默认值要标出来; 超过 40 项要**如实计数**, 不静默截断。
#[test]
fn test_changed_lines_marks_default_and_counts_overflow() {
    let one = ParamChange {
        key: "spacing_pct".to_string(),
        name: "间距百分比".to_string(),
        before: Some("0.01".to_string()),
        after: "0.02".to_string(),
        before_is_default: true,
    };
    let text = changed_lines(&[one]);
    assert!(text.contains("间距百分比 (spacing_pct): 0.01(清单默认, 未保存过) → 0.02"), "{text}");

    let none_before = ParamChange {
        key: "start_price".to_string(),
        name: "开始价格".to_string(),
        before: None,
        after: "123".to_string(),
        before_is_default: true,
    };
    assert!(changed_lines(&[none_before]).contains("未设置 → 123"));

    let many: Vec<ParamChange> = (0..MAX_CHANGED_LINES + 3)
        .map(|i| ParamChange {
            key: format!("k{i}"),
            name: format!("键{i}"),
            before: Some("1".to_string()),
            after: "2".to_string(),
            before_is_default: false,
        })
        .collect();
    let text = changed_lines(&many);
    assert!(text.contains("(另有 3 项改动未列出)"), "截断必须报数: {text}");
    assert!(!text.contains("k43"), "超限的行不该出现: {text}");
}

// ---- N3-B: 评估端点的离线守卫 ----

/// 评估端点的同步校验: 没有改动 / 指标不是对象 / 未知 id 都在联网前拦掉;
/// 正常的请求在没配密钥时 403 `need_keys`(证明它真的走到了 provider 的密钥前置)。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_ai_assess_offline_guards() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let saved =
        ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].map(|k| std::env::var(k).ok());
    for k in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"] {
        std::env::remove_var(k);
    }

    let root = tmp_root("aiassess");
    let port = boot(root).await;

    let metric = serde_json::json!({"pair": "BTCUSDT", "interval": "1h", "days": 90,
        "equity_change_pct": 1.0, "max_drawdown_pct": 2.0, "total_trades": 3});
    let changed = serde_json::json!([{"key": "spacing_pct", "name": "间距百分比",
        "before": "0.01", "after": "0.02", "before_is_default": false}]);

    // 没有改动 → 400 nothing_changed(评估"什么都没改"没有意义)。
    let body = serde_json::json!({"changed": [], "base_metrics": metric, "new_metrics": metric});
    let res = raw(
        port,
        "POST",
        "/api/strategies/paired_grid/ai-assess?token=tok-ok",
        Some(&body.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("nothing_changed"), "{res}");

    // 指标不是对象 → 400 bad_metrics。
    let body = serde_json::json!({"changed": changed, "base_metrics": [1], "new_metrics": metric});
    let res = raw(
        port,
        "POST",
        "/api/strategies/paired_grid/ai-assess?token=tok-ok",
        Some(&body.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 400, "{res}");
    assert!(body_of(&res).contains("bad_metrics"), "{res}");

    // 未知 id → 404(与 ai-edit 同口径)。
    let body =
        serde_json::json!({"changed": changed, "base_metrics": metric, "new_metrics": metric});
    let res = raw(
        port,
        "POST",
        "/api/strategies/no-such-x/ai-assess?token=tok-ok",
        Some(&body.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 404, "{res}");

    // 正常请求 + 没配密钥 → 403 need_keys(同步校验全过了才会走到这一步)。
    let res = raw(
        port,
        "POST",
        "/api/strategies/paired_grid/ai-assess?token=tok-ok",
        Some(&body.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 403, "{res}");
    assert!(body_of(&res).contains("need_keys"), "{res}");

    // 未带 token → 401(与其他策略端点一致)。
    let res =
        raw(port, "POST", "/api/strategies/paired_grid/ai-assess", Some(&body.to_string())).await;
    assert_eq!(status_of(&res), 401, "{res}");

    for (k, v) in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].iter().zip(saved) {
        if let Some(val) = v {
            std::env::set_var(k, val);
        } else {
            std::env::remove_var(k);
        }
    }
}

/// 多轮历史从请求进到 provider 提示词(用离线端点验证: 历史非法也只是 400/403, 不影响同步校验)。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn test_ai_edit_params_accepts_history_field() {
    let _g = crate::commands::test_util::ENV_LOCK.lock().unwrap();
    let saved =
        ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].map(|k| std::env::var(k).ok());
    for k in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"] {
        std::env::remove_var(k);
    }

    let root = tmp_root("aihist");
    let port = boot(root).await;
    // 带 history 的请求必须**解析得进去**(serde 不得因为多了一个字段就 400),
    // 一路走到"没配密钥"的 403 —— 这正好证明它进了 params 分支而不是被字段校验挡住。
    let body = serde_json::json!({
        "instruction": "再保守一点",
        "mode": "params",
        "history": "用户: 保守点\nAI: 我把间距放宽到 0.02 了。"
    });
    let res = raw(
        port,
        "POST",
        "/api/strategies/paired_grid/ai-edit?token=tok-ok",
        Some(&body.to_string()),
    )
    .await;
    assert_eq!(status_of(&res), 403, "{res}");
    assert!(body_of(&res).contains("need_keys"), "{res}");

    for (k, v) in ["RICOW_AI_API_KEY", "RICOW_AI_BASE_URL", "RICOW_AI_MODEL"].iter().zip(saved) {
        if let Some(val) = v {
            std::env::set_var(k, val);
        } else {
            std::env::remove_var(k);
        }
    }
}

// ── 054 T41: 验证回测必须跑在**用户填的本金**上 ──────────────────────────────────
//
// 真跑发现的缺陷: `verify_generated` 此前 `cash: None`, 一律按默认 10 万跑;
// 而给模型的提示词说的是"用户准备投入 {initial_cash} USDT"(spec §四-2)。
// 用户填 1 万却看到 10 万的成交与盈亏 —— 仓位拒单、手续费占比、网格能否铺满,
// 在小资金下完全是另一回事。跑回测要联网(测不了), 所以把"本金真带进去了"
// 用纯函数 [`super::verify_spec`] 钉住。
#[test]
fn test_verify_spec_runs_on_users_own_cash_not_the_default() {
    use std::path::Path;

    let spec = super::verify_spec(
        Path::new("/tmp/root"),
        "zz_v",
        "-- lua",
        "ETHUSDT",
        "spot",
        30,
        "1h",
        10000.0,
        None, // 现货没有杠杆上限
    );
    assert_eq!(spec.cash, Some(10000.0), "验证回测必须用用户填的本金, 不能用默认 10 万: {spec:?}");
    // 同一份**未落盘**的代码走 raw_code 分支(刻意不落盘再回测)。
    assert!(spec.raw_code.is_some(), "验证必须跑 raw_code 分支: {spec:?}");
    assert_eq!(spec.days, 30);
    assert_eq!(spec.interval, "1h");
    // 生成弹窗选的周期要压过 Lua 自己的 fallback。
    assert!(spec.interval_explicit);

    // 小资金也要照实传(500 U 铺不开 12 档网格会如实拒单 —— 这正是用户该知道的)。
    let small = super::verify_spec(
        Path::new("/tmp/root"),
        "zz_v",
        "-- lua",
        "ETHUSDT",
        "spot",
        30,
        "1h",
        500.0,
        None,
    );
    assert_eq!(small.cash, Some(500.0), "小资金同样要用用户填的数: {small:?}");
}

// ── 055 B2: 验证回测必须跑在**档位的杠杆上限**下 ──────────────────────────────────
//
// 与上面本金那条**同一个 bug 的另一半**: `verify_spec` 此前 `max_leverage: None`
// (→ 引擎默认 10x), 而保存时档位写的是 2/3/5x。于是合约策略**生成时看到的成绩**
// 与**保存后点回测看到的成绩**不是一回事 —— P1「生成即验证」在合约场景等于白跑。
#[test]
fn test_verify_spec_carries_the_tier_leverage_cap() {
    use std::path::Path;

    let fut = super::verify_spec(
        Path::new("/tmp/root"),
        "zz_v",
        "-- lua",
        "ETHUSDT",
        "futures",
        90,
        "1h",
        10000.0,
        Some(2.0), // 保守档上限
    );
    assert_eq!(fut.max_leverage, Some(2.0), "验证回测必须按档位上限跑: {fut:?}");
    // **实际**杠杆仍是默认 1x —— 与保存后的 TOML 口径一致(那边也只写 max_leverage)。
    // 若这里也写 2.0, 就等于替用户开了 2 倍杠杆(055 B1)。
    assert_eq!(fut.leverage, None, "验证回测不该替用户指定实际杠杆: {fut:?}");

    // 激进档 5x 同样照实传 —— 上限是 5x, 不是"就用 5x"。
    let agg = super::verify_spec(
        Path::new("/tmp/root"),
        "zz_v",
        "-- lua",
        "ETHUSDT",
        "futures",
        90,
        "1h",
        10000.0,
        Some(5.0),
    );
    assert_eq!(agg.max_leverage, Some(5.0), "激进档上限要照实带进去: {agg:?}");
    assert_eq!(agg.leverage, None, "同上: 实际杠杆不代用户决定: {agg:?}");

    // 现货恒为 None —— 现货没有杠杆这件事要说清楚, 不能残留一个数字。
    let spot = super::verify_spec(
        Path::new("/tmp/root"),
        "zz_v",
        "-- lua",
        "ETHUSDT",
        "spot",
        90,
        "1h",
        10000.0,
        None,
    );
    assert_eq!(spot.max_leverage, None, "现货不该带杠杆上限: {spot:?}");
}

// ── 055 B1: 档位的杠杆**上限**必须落进 `max_leverage`, 不是 `leverage` ──────────────
//
// 这是本轮审核发现的最严重一条。`leverage` 是**实际杠杆**, 而 `commands/run.rs` 启动
// 实盘/demo 时读的正是 `[backtest].leverage`, 会把它**真的设到交易所**上去
// (`ex.prepare(pair, leverage, ...)`)。写它 = 用户点了个「保守」, 平台替他开 2 倍杠杆。
#[test]
fn test_save_request_writes_tier_cap_as_max_leverage_not_as_actual_leverage() {
    // 前端现在的落盘体(见 strategies.js 保存段): 带 max_leverage, 不带 leverage。
    let body = serde_json::json!({
        "name": "zz_lev",
        "market": "futures",
        "pair": "ETHUSDT",
        "code": "function on_tick(ctx) end",
        "max_leverage": 2.0,
    });
    let req: super::SaveRequest = serde_json::from_value(body).expect("请求体应能解析");
    assert_eq!(req.max_leverage, Some(2.0), "档位上限应落在 max_leverage: {req:?}");
    assert_eq!(
        req.leverage, None,
        "生成路径**不得**传 leverage(实际杠杆): 它会一路走到交易所, 等于替用户开杠杆"
    );
}
