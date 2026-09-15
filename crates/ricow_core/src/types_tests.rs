#[cfg(test)]
mod tests {
    use crate::*;
    use chrono::Utc;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    #[test]
    fn test_orderbook_empty() {
        let ob = OrderBook::default();
        assert!(ob.best_bid().is_none());
        assert!(ob.best_ask().is_none());
        assert!(ob.mid_price().is_none());
        assert!(ob.spread().is_none());
    }

    #[test]
    fn test_orderbook_mid_price() {
        let ob = OrderBook {
            bids: vec![PriceLevel { price: dec!(100.0), size: dec!(1.0) }],
            asks: vec![PriceLevel { price: dec!(102.0), size: dec!(1.0) }],
            timestamp: Utc::now(),
        };
        assert_eq!(ob.mid_price(), Some(dec!(101.0)));
        assert_eq!(ob.spread(), Some(dec!(2.0)));
        assert_eq!(ob.best_bid().unwrap().price, dec!(100.0));
        assert_eq!(ob.best_ask().unwrap().price, dec!(102.0));
    }

    #[test]
    fn test_balance_total() {
        let b = Balance { asset: "USDC".into(), free: dec!(100.0), locked: dec!(20.0) };
        assert_eq!(b.total(), dec!(120.0));
    }

    #[test]
    fn test_order_request_limit() {
        let req = OrderRequest::new_limit("ETH", OrderSide::Buy, dec!(3000.0), dec!(0.01));
        assert_eq!(req.pair, "ETH");
        assert_eq!(req.side, OrderSide::Buy);
        assert_eq!(req.order_type, OrderType::Limit);
        assert_eq!(req.price, Some(dec!(3000.0)));
        assert_eq!(req.size, dec!(0.01));
        assert!(!req.reduce_only);
    }

    #[test]
    fn test_order_request_market() {
        let req = OrderRequest::new_market("BTC", OrderSide::Sell, dec!(0.1));
        assert_eq!(req.order_type, OrderType::Market);
        assert_eq!(req.price, None);
    }

    #[test]
    fn test_order_request_reduce_only() {
        let req = OrderRequest::new_limit("ETH", OrderSide::Sell, dec!(3100.0), dec!(0.01))
            .with_reduce_only(true);
        assert!(req.reduce_only);
    }

    #[test]
    fn test_order_side_display() {
        assert_eq!(OrderSide::Buy.to_string(), "buy");
        assert_eq!(OrderSide::Sell.to_string(), "sell");
    }

    #[test]
    fn test_order_type_display() {
        assert_eq!(OrderType::Limit.to_string(), "limit");
        assert_eq!(OrderType::Market.to_string(), "market");
    }

    #[test]
    fn test_order_status_display() {
        assert_eq!(OrderStatus::Open.to_string(), "open");
        assert_eq!(OrderStatus::Filled.to_string(), "filled");
        assert_eq!(OrderStatus::PartiallyFilled.to_string(), "partially_filled");
        assert_eq!(OrderStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(OrderStatus::Rejected.to_string(), "rejected");
        assert_eq!(OrderStatus::Expired.to_string(), "expired");
    }

    #[test]
    fn test_orderbook_sorted() {
        let ob = OrderBook::new_sorted(
            vec![
                PriceLevel { price: dec!(99.0), size: dec!(1.0) },
                PriceLevel { price: dec!(101.0), size: dec!(2.0) },
                PriceLevel { price: dec!(100.0), size: dec!(3.0) },
            ],
            vec![
                PriceLevel { price: dec!(103.0), size: dec!(1.0) },
                PriceLevel { price: dec!(101.0), size: dec!(2.0) },
                PriceLevel { price: dec!(102.0), size: dec!(3.0) },
            ],
        );
        assert_eq!(ob.best_bid().unwrap().price, dec!(101.0));
        assert_eq!(ob.best_ask().unwrap().price, dec!(101.0));
        assert_eq!(ob.mid_price(), Some(dec!(101.0)));
    }

    #[test]
    fn test_serde_roundtrip_order_side() {
        let side = OrderSide::Buy;
        let json = serde_json::to_string(&side).unwrap();
        let parsed: OrderSide = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, side);
    }

    #[test]
    fn test_serde_roundtrip_market() {
        let m = Market {
            symbol: "ETH".into(),
            base_asset: "ETH".into(),
            quote_asset: "USD".into(),
            is_perpetual: true,
            min_size: Decimal::new(1, 4),
            tick_size: Decimal::new(1, 2),
            step_size: None,
            min_notional: None,
            max_leverage: Some(50),
            margin_mode: None,
            is_delisted: false,
        };
        let json = serde_json::to_string(&m).unwrap();
        let parsed: Market = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn test_market_min_notional_backward_compat() {
        // 旧缓存/旧序列化数据无 min_notional 字段 → 解析为 None, 不报错。
        let legacy = r#"{
            "symbol": "ETHUSDT", "base_asset": "ETH", "quote_asset": "USDT",
            "is_perpetual": false, "min_size": "0.001", "tick_size": "0.01",
            "max_leverage": null, "margin_mode": null, "is_delisted": false
        }"#;
        let m: Market = serde_json::from_str(legacy).expect("旧数据应可反序列化");
        assert_eq!(m.min_notional, None);
        assert_eq!(m.step_size, None);

        // 带 min_notional 的数据可往返; None 时不序列化该字段 (与 step_size 同策略)。
        let m2 = Market { min_notional: Some(Decimal::from(10)), ..m.clone() };
        let json = serde_json::to_string(&m2).unwrap();
        assert!(json.contains("min_notional"));
        assert_eq!(serde_json::from_str::<Market>(&json).unwrap(), m2);
        let json_none = serde_json::to_string(&m).unwrap();
        assert!(!json_none.contains("min_notional"), "None 不应出现该字段: {json_none}");
    }

    #[test]
    fn test_parse_pair_with_prefix() {
        assert_eq!(parse_pair("binance:ETH"), ("binance", "ETH"));
        assert_eq!(parse_pair("bn:BTC"), ("bn", "BTC"));
        assert_eq!(parse_pair("binance:SOL"), ("binance", "SOL"));
    }

    #[test]
    fn test_parse_pair_without_prefix() {
        assert_eq!(parse_pair("ETH"), ("", "ETH"));
        assert_eq!(parse_pair("BTC"), ("", "BTC"));
        assert_eq!(parse_pair("SOL-PERP"), ("", "SOL-PERP"));
    }

    #[test]
    fn test_parse_pair_edge_cases() {
        assert_eq!(parse_pair(""), ("", ""));
        assert_eq!(parse_pair("binance:"), ("binance", ""));
        assert_eq!(parse_pair("binance:BTC:USD"), ("binance", "BTC:USD"));
    }
}
