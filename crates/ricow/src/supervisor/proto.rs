//! 控制通道协议: 单行 JSON 请求 / 单行 JSON 响应 (008)。
//!
//! 通道 = 仅绑定 127.0.0.1 的 TCP; 每个请求必须携带 `token` (来自 `run/daemon.json`)。
//! 协议保持极简: 一行一个 JSON 对象, 便于人工用 nc 复现与单测。

use serde::{Deserialize, Serialize};

/// 请求指令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// 连通性与版本探测
    Ping,
    /// 列出当前由 daemon 持有的实例
    List,
    /// 启动策略 (`live=true` = 命令行显式要求实盘; 仍须 TOML `live_enabled=true` 才真进实盘)
    Start {
        name: String,
        #[serde(default)]
        live: bool,
        /// demo(测试网)模式: 真实调用币安测试网下单接口, 不涉真实资金, 故不适用实盘三判据
        #[serde(default)]
        demo: bool,
        /// 实盘**已由用户逐字确认**(019 T030): 交互确认只在用户终端发生, 不进入子进程 stdin。
        /// 缺此确认时 daemon 拒绝实盘启动(不静默降级)。
        #[serde(default)]
        confirmed: bool,
    },
    /// 优雅停止策略 (下发停机指令并等待; `close_all=true` = 停机时平掉策略持仓)
    Stop {
        name: String,
        #[serde(default)]
        close_all: bool,
    },
    /// 单实例详情 (live 视图 + 台账)
    Info { name: String },
    /// 停止 daemon (先优雅停全部策略)
    Shutdown,
}

/// 带令牌的请求信封 (线上格式)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub token: String,
    #[serde(flatten)]
    pub request: Request,
}

/// 响应 (线上格式): `ok=false` 时 `error` 必填。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl Response {
    pub fn ok(data: Option<serde_json::Value>) -> Self {
        Self { ok: true, error: None, data }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()), data: None }
    }
}

/// 实例视图 (daemon 返回给 CLI; 只含事实字段)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceView {
    pub name: String,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reason: Option<String>,
}

/// `stop` 的返回: 如实描述是否优雅退出 (不夸大清理结果)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StopReport {
    pub name: String,
    /// 是否观测到进程退出
    pub exited: bool,
    /// 是否经停机指令优雅退出 (false = 超时未退)
    pub graceful: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub waited_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 名字存在但**本来就没在跑** (027): true 表示本次未下发停机指令、未等待退出、未写台账、
    /// 未触发清理 —— 回执只陈述"该策略未在运行 (无需停止)"这一个事实。
    /// `#[serde(default)]` 使缺该字段的旧 daemon 回执解析为 false (行为与今日一致, 不产生反向假阴性)。
    #[serde(default)]
    pub already_stopped: bool,
}

pub fn encode<T: Serialize>(value: &T) -> String {
    let mut s = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    s.push('\n');
    s
}

pub fn decode_envelope(line: &str) -> Result<Envelope, String> {
    serde_json::from_str::<Envelope>(line.trim()).map_err(|e| format!("请求解析失败: {e}"))
}

pub fn decode_response(line: &str) -> Result<Response, String> {
    serde_json::from_str::<Response>(line.trim()).map_err(|e| format!("响应解析失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_demo_field_roundtrip_and_backward_compat() {
        // demo 字段需可序列化/反序列化
        let e = Envelope {
            token: "t".into(),
            request: Request::Start { name: "g".into(), live: false, demo: true, confirmed: false },
        };
        let js = serde_json::to_string(&e).expect("ser");
        assert!(js.contains("\"demo\":true"));
        assert_eq!(serde_json::from_str::<Envelope>(&js).expect("de"), e);
        // 兼容: 老版本 CLI 发来的请求不含 demo 字段 → 默认 false (不阻塞版本混跑)
        let old = r#"{"token":"t","cmd":"start","name":"g","live":false}"#;
        let parsed: Envelope = serde_json::from_str(old).expect("旧格式必须仍能解析");
        assert_eq!(
            parsed.request,
            Request::Start { name: "g".into(), live: false, demo: false, confirmed: false }
        );
    }

    #[test]
    fn request_roundtrip() {
        for req in [
            Request::Ping,
            Request::List,
            Request::Start { name: "grid".into(), live: true, demo: false, confirmed: true },
            Request::Stop { name: "grid".into(), close_all: true },
            Request::Info { name: "grid".into() },
            Request::Shutdown,
        ] {
            let env = Envelope { token: "tok".into(), request: req.clone() };
            let line = encode(&env);
            assert!(line.ends_with('\n'), "线上格式须以换行结尾");
            let back = decode_envelope(&line).expect("应可解析");
            assert_eq!(back, env, "请求应无损往返");
        }
    }

    #[test]
    fn start_stop_flags_default_false_for_old_clients() {
        // 缺省字段按 false 解析: 旧客户端/手工 nc 请求不会意外进实盘或触发平仓
        let env = decode_envelope(r#"{"token":"t","cmd":"start","name":"g"}"#).unwrap();
        assert_eq!(
            env.request,
            Request::Start { name: "g".into(), live: false, demo: false, confirmed: false }
        );
        // 实盘确认字段: 旧请求缺省为 false(= 未确认 → daemon 拒绝实盘, 不静默降级)
        let env_c = decode_envelope(
            r#"{"token":"t","cmd":"start","name":"g","live":true,"confirmed":true}"#,
        )
        .unwrap();
        assert_eq!(
            env_c.request,
            Request::Start { name: "g".into(), live: true, demo: false, confirmed: true }
        );
        let env_old =
            decode_envelope(r#"{"token":"t","cmd":"start","name":"g","live":true}"#).unwrap();
        assert_eq!(
            env_old.request,
            Request::Start { name: "g".into(), live: true, demo: false, confirmed: false }
        );
        let env2 = decode_envelope(r#"{"token":"t","cmd":"stop","name":"g"}"#).unwrap();
        assert_eq!(env2.request, Request::Stop { name: "g".into(), close_all: false });
        // 显式携带时如实解析
        let env3 =
            decode_envelope(r#"{"token":"t","cmd":"stop","name":"g","close_all":true}"#).unwrap();
        assert_eq!(env3.request, Request::Stop { name: "g".into(), close_all: true });
    }

    #[test]
    fn envelope_shape_is_flat_json() {
        let env = Envelope {
            token: "tok".into(),
            request: Request::Stop { name: "g".into(), close_all: false },
        };
        let v: serde_json::Value = serde_json::from_str(&encode(&env)).unwrap();
        assert_eq!(v["cmd"], "stop");
        assert_eq!(v["name"], "g");
        assert_eq!(v["token"], "tok");
    }

    #[test]
    fn bad_json_and_unknown_cmd_rejected() {
        assert!(decode_envelope("{ not json").is_err());
        assert!(decode_envelope(r#"{"token":"t","cmd":"nope"}"#).is_err());
    }

    #[test]
    fn response_roundtrip() {
        let ok = Response::ok(Some(serde_json::json!({"instances": []})));
        let back = decode_response(&encode(&ok)).unwrap();
        assert!(back.ok);
        assert_eq!(back.data, Some(serde_json::json!({"instances": []})));

        let err = Response::err("daemon 未运行");
        let back = decode_response(&encode(&err)).unwrap();
        assert!(!back.ok);
        assert_eq!(back.error.as_deref(), Some("daemon 未运行"));
    }

    #[test]
    fn stop_report_roundtrip() {
        let r = StopReport {
            name: "grid".into(),
            exited: true,
            graceful: true,
            exit_code: Some(0),
            waited_ms: 123,
            note: Some("策略未实现清理 (on_stop): 如仍有挂单/持仓请手工处理".into()),
            already_stopped: false,
        };
        let s = encode(&r);
        let back: StopReport = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    /// 027: `already_stopped` 必须**始终序列化**(新 daemon → 新 CLI 靠它区分"本来就没在跑"
    /// 与"刚刚停掉"); 而**缺该字段的旧格式**必须解析为 false(新 CLI 遇旧 daemon 行为不变)。
    #[test]
    fn stop_report_already_stopped_defaults_false_for_old_format() {
        let new = StopReport {
            name: "grid".into(),
            exited: true,
            graceful: true,
            exit_code: None,
            waited_ms: 0,
            note: Some("该策略未在运行 (无需停止)".into()),
            already_stopped: true,
        };
        let s = encode(&new);
        assert!(s.contains("\"already_stopped\":true"), "新字段必须出现在线上格式里: {s}");

        let old =
            "{\"name\":\"grid\",\"exited\":true,\"graceful\":true,\"waited_ms\":0,\"note\":\"n\"}";
        let back: StopReport = serde_json::from_str(old).expect("旧格式应可解析");
        assert!(!back.already_stopped, "缺字段必须缺省为 false");
    }
}
