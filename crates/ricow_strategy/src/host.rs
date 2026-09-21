//! 宿主服务接缝 (028 T016): 策略侧只依赖这几个窄 trait, 实现与网络栈留在引擎侧。
//!
//! 依赖方向: `ricow_engine`(实现) → `ricow_strategy`(定义) —— 与既有方向一致, 无环。
//! 为什么这么切: `ricow_strategy` 里**没有** reqwest/url/tokio-runtime 之类的网络依赖,
//! 若把 `data:*` / `http:*` 的实现写在这里, 要么给该 crate 塞网络依赖, 要么形成
//! strategy → engine 的反向依赖。接缝把"谁来取数"留在引擎, 策略侧只表达"我要什么"。
//!
//! 同步语义: Lua 在策略回调里同步调用这些方法; 由引擎侧实现负责把 async 桥接回同步
//! (LiveContext 早就是这么做的)。策略侧因此不需要 async 运行时。

use ricow_core::{CoreError, CoreResult, Kline, SeriesKey};

use crate::series::SeriesDecl;

/// 策略可见的宿主服务: 取数 / 取历史 / HTTP。
pub trait HostServices: Send + Sync {
    /// 按声明装载一条序列的 K 线(尾窗 + 装配层补的预热段)。
    ///
    /// 失败语义由调用方决定: `data:series` 直接报错; 增量回补失败则由引擎标 `stale` 后
    /// 返回最后一版可用数据。
    fn load_series(&self, decl: &SeriesDecl) -> CoreResult<Vec<Kline>>;

    /// 读某序列的本地历史(升序)。**只读本地缓存**, 不发网络请求 ——
    /// 回测可复现的底线(拍板 D3): 缺数据就让策略拿到明确的"缺数据"错误。
    fn history(&self, key: &SeriesKey, limit: Option<usize>) -> CoreResult<Vec<Kline>>;

    /// 策略自取 HTTP GET(仅 GET;C 的 URL 由策略决定, 平台不限制域名)。
    ///
    /// 由宿主在独立线程 + 独立运行时的墙钟超时里执行, 并限制响应体大小 ——
    /// 策略沙箱的指令预算对"阻塞等待"零约束, 所以约束必须落在宿主侧(plan d13)。
    fn http_get(&self, url: &str) -> CoreResult<String>;
}

/// 未装配宿主: 任何取数/HTTP 调用都返回**明确错误**, 不静默给空数据。
///
/// 用途: 单元测试/回测里不需要数据服务的场景(既有策略只读 `ctx:*` 快照), 以及
/// "有人忘了注入宿主"时给出可读的诊断, 而不是让策略看到一条空序列。
#[derive(Debug, Default, Clone, Copy)]
pub struct NullHost;

const NO_HOST: &str = "数据服务未装配: 当前运行环境没有注入宿主(点此检查装配层是否调用 set_host)";

impl HostServices for NullHost {
    fn load_series(&self, _decl: &SeriesDecl) -> CoreResult<Vec<Kline>> {
        Err(CoreError::InvalidArgument(format!("data:series 不可用 —— {NO_HOST}")))
    }

    fn history(&self, _key: &SeriesKey, _limit: Option<usize>) -> CoreResult<Vec<Kline>> {
        Err(CoreError::InvalidArgument(format!("data:history 不可用 —— {NO_HOST}")))
    }

    fn http_get(&self, _url: &str) -> CoreResult<String> {
        Err(CoreError::InvalidArgument(format!("http:get 不可用 —— {NO_HOST}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ricow_core::Interval;

    fn key() -> SeriesKey {
        SeriesKey::new("yahoo", "QQQ", Interval::D1).unwrap()
    }

    #[test]
    fn test_null_host_errors_are_actionable() {
        let h = NullHost;
        let decl = SeriesDecl::new("q", key());
        let e = h.load_series(&decl).unwrap_err().to_string();
        assert!(e.contains("data:series 不可用"), "{e}");
        assert!(e.contains("数据服务未装配"), "{e}");
        assert!(h.history(&key(), None).unwrap_err().to_string().contains("data:history 不可用"));
        assert!(h
            .http_get("https://example.com")
            .unwrap_err()
            .to_string()
            .contains("http:get 不可用"));
    }
}
