//! CLI 侧控制通道客户端 (008): 读 `run/daemon.json` → 连接 127.0.0.1 → 携带 token 请求。

use std::path::Path;

use ricow_core::{CoreError, CoreResult};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::supervisor::ledger::DaemonInfo;
use crate::supervisor::proto::{self, Request, Response};

/// 已连接的控制通道。
pub struct Client {
    pub(crate) info: DaemonInfo,
}

impl Client {
    /// 连接本机 daemon; 无 `daemon.json` 或连不上时返回明确错误 (不自动拉起 daemon)。
    pub async fn connect(root: &Path) -> CoreResult<Self> {
        let info = crate::supervisor::ledger::read_daemon_info(root).ok_or_else(|| {
            CoreError::InvalidArgument(
                "daemon 未运行 (无 run/daemon.json); 先执行 ricow daemon start".into(),
            )
        })?;
        let addr = format!("127.0.0.1:{}", info.port);
        match TcpStream::connect(&addr).await {
            Ok(_) => Ok(Self { info }),
            Err(e) => Err(CoreError::Network(format!(
                "无法连接 daemon ({addr}): {e}; 若 daemon 已退出, 可删除 run/daemon.json 后重启"
            ))),
        }
    }

    pub fn port(&self) -> u16 {
        self.info.port
    }

    pub fn pid(&self) -> u32 {
        self.info.pid
    }

    /// 发一条请求并读一条响应。
    pub async fn call(&self, request: Request) -> CoreResult<Response> {
        let env = proto::Envelope { token: self.info.token.clone(), request };
        let addr = format!("127.0.0.1:{}", self.info.port);
        let stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| CoreError::Network(format!("连接 daemon 失败 ({addr}): {e}")))?;
        let (reader, mut writer) = stream.into_split();
        writer
            .write_all(proto::encode(&env).as_bytes())
            .await
            .map_err(|e| CoreError::Network(format!("发送请求失败: {e}")))?;
        writer.flush().await.ok();

        let mut lines = BufReader::new(reader).lines();
        let line = lines
            .next_line()
            .await
            .map_err(|e| CoreError::Network(format!("读取响应失败: {e}")))?
            .ok_or_else(|| CoreError::Network("daemon 未返回响应 (连接被关闭)".into()))?;
        proto::decode_response(&line).map_err(CoreError::Parse)
    }

    /// 发请求并要求 `ok=true`, 否则返回 daemon 的错误信息。
    pub async fn call_ok(&self, request: Request) -> CoreResult<serde_json::Value> {
        let resp = self.call(request).await?;
        if resp.ok {
            Ok(resp.data.unwrap_or(serde_json::Value::Null))
        } else {
            Err(CoreError::InvalidArgument(resp.error.unwrap_or_else(|| "daemon 返回失败".into())))
        }
    }
}
