//! Binance Simple Earn 活期理财 SAPI 端点 (申购/赎回/持仓/产品列表)。
//!
//! 全部走签名请求 (HMAC-SHA256, 与现货 REST 同基建)。金额字符串化统一
//! 截断到 8 位小数并去尾零 (币安 SAPI 对多余精度直接拒单)。

use std::str::FromStr;

use ricow_core::{CoreError, CoreResult};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::client::BinanceClient;

/// 金额序列化: 向下截断 8 位小数 + 去尾零 ("10.500000000" → "10.5")。
pub(crate) fn format_earn_amount(d: Decimal) -> String {
    let truncated = d.trunc_with_scale(8);
    let s = truncated.to_string();
    match s.find('.') {
        Some(dot) => {
            let int_part = &s[..dot];
            let frac = s[dot + 1..].trim_end_matches('0');
            if frac.is_empty() {
                int_part.to_string()
            } else {
                format!("{int_part}.{frac}")
            }
        }
        None => s,
    }
}

/// 从 `{"rows":[...]}` 响应体取 rows 数组。
fn rows_of(resp: &Value) -> CoreResult<&Vec<Value>> {
    resp["rows"].as_array().ok_or_else(|| CoreError::Parse("simple-earn 响应缺 rows".into()))
}

impl BinanceClient {
    /// `GET /sapi/v1/simple-earn/flexible/list?asset=X` → 可申购活期产品列表
    /// (元素为 `(productId, 最新年化利率)`, 按返回顺序)。
    pub(crate) async fn earn_flexible_list(
        &self,
        asset: &str,
    ) -> CoreResult<Vec<(String, Decimal)>> {
        let params = vec![("asset".to_string(), asset.to_uppercase())];
        let resp = self.signed_get("/sapi/v1/simple-earn/flexible/list", &params).await?;
        let rows = rows_of(&resp)?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let product_id = r["productId"].as_str()?.to_string();
                let apr = r["latestAnnualPercentageRate"]
                    .as_str()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or_default();
                Some((product_id, apr))
            })
            .collect())
    }

    /// `POST /sapi/v1/simple-earn/flexible/subscribe` (sourceAccount 默认 SPOT)。
    ///
    /// 响应 `success:false` 视为失败 (币安在 HTTP 200 下也可能拒绝申购)。
    pub(crate) async fn earn_subscribe(&self, product_id: &str, amount: Decimal) -> CoreResult<()> {
        let params = vec![
            ("productId".to_string(), product_id.to_string()),
            ("amount".to_string(), format_earn_amount(amount)),
        ];
        let resp = self.signed_post("/sapi/v1/simple-earn/flexible/subscribe", &params).await?;
        if resp.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(CoreError::Exchange(format!("Simple Earn 申购被拒: {resp}")));
        }
        Ok(())
    }

    /// `POST /sapi/v1/simple-earn/flexible/redeem` (部分赎回; 到账时点由上层处理)。
    pub(crate) async fn earn_redeem(&self, product_id: &str, amount: Decimal) -> CoreResult<()> {
        let params = vec![
            ("productId".to_string(), product_id.to_string()),
            ("amount".to_string(), format_earn_amount(amount)),
        ];
        let resp = self.signed_post("/sapi/v1/simple-earn/flexible/redeem", &params).await?;
        if resp.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(CoreError::Exchange(format!("Simple Earn 赎回被拒: {resp}")));
        }
        Ok(())
    }

    /// `GET /sapi/v1/simple-earn/flexible/position?asset=X` → 该资产全部活期
    /// 持仓的 `amount` 之和 (在理财数量)。
    pub(crate) async fn earn_position_amount(&self, asset: &str) -> CoreResult<Decimal> {
        let params = vec![("asset".to_string(), asset.to_uppercase())];
        let resp = self.signed_get("/sapi/v1/simple-earn/flexible/position", &params).await?;
        let rows = rows_of(&resp)?;
        Ok(rows
            .iter()
            .filter_map(|r| r["amount"].as_str().and_then(|s| Decimal::from_str(s).ok()))
            .fold(Decimal::ZERO, |acc, x| acc + x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_earn_amount() {
        let d = |s: &str| Decimal::from_str(s).unwrap();
        assert_eq!(format_earn_amount(d("10.500000000")), "10.5");
        assert_eq!(format_earn_amount(d("3000.0")), "3000");
        assert_eq!(format_earn_amount(d("0.123456789")), "0.12345678"); // 截断非四舍五入
        assert_eq!(format_earn_amount(d("100")), "100");
        assert_eq!(format_earn_amount(Decimal::ZERO), "0");
    }

    #[test]
    fn test_parse_flexible_list_rows() {
        // 与币安 list 响应同构的样本: 取 productId + APR, 缺字段行跳过, 不 panic。
        let resp = serde_json::json!({
            "page": 1, "pages": 1, "total": 2,
            "rows": [
                {"productId": "USDT001", "asset": "USDT", "latestAnnualPercentageRate": "0.05000000", "tier": 0},
                {"productId": "USDT002", "asset": "USDT"}
            ]
        });
        let rows = rows_of(&resp).unwrap();
        let parsed: Vec<(String, Decimal)> = rows
            .iter()
            .filter_map(|r| {
                let pid = r["productId"].as_str()?.to_string();
                let apr = r["latestAnnualPercentageRate"]
                    .as_str()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or_default();
                Some((pid, apr))
            })
            .collect();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "USDT001");
        assert_eq!(parsed[0].1.to_string(), "0.05000000");
        assert_eq!(parsed[1].1, Decimal::ZERO);
    }

    #[test]
    fn test_rows_of_missing_is_err() {
        let resp = serde_json::json!({"code": -2015, "msg": "Invalid API-key"});
        assert!(rows_of(&resp).is_err());
    }

    #[test]
    fn test_position_amount_sum() {
        let resp = serde_json::json!({
            "rows": [
                {"productId": "USDT001", "asset": "USDT", "amount": "100.5"},
                {"productId": "USDT002", "asset": "USDT", "amount": "0.25"}
            ]
        });
        let sum = rows_of(&resp)
            .unwrap()
            .iter()
            .filter_map(|r| r["amount"].as_str().and_then(|s| Decimal::from_str(s).ok()))
            .fold(Decimal::ZERO, |acc, x| acc + x);
        assert_eq!(sum, Decimal::from_str("100.75").unwrap());
    }
}
