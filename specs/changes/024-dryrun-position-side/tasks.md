# 024 任务分解

依据 [plan.md](./plan.md)(D1–D9)与 [spec.md](./spec.md)(FR-001..008 / SC-001..005)。

- [x] T1 抽取纯函数 `apply_fill_to_net_position`(D2):`context.rs` 内把 `apply_position_change` 的 match 主体搬进 `fn apply_fill_to_net_position(entry: &mut Position, side: OrderSide, size: Decimal, fill_price: Decimal) -> Decimal`(**仅抽取, 不修复**), `apply_position_change` 改为调用它
- [x] T2 新增单测 ≥5 条(FR-007):`context.rs` `mod tests` 覆盖 ①平多后反向开空 ②平空后反向开多 ③平多后同向再开多(开仓价重算) ④部分平仓保留剩余方向与开仓价(含已实现盈亏) ⑤反手(成交量 > 当前持仓量)后的 `side`/`size`
- [x] T3 **红验证**(SC-002 上半):`cargo test -p ricow_strategy` 跑出失败, 记录"①②③⑤ 红 / ④ 绿"的输出留档 —— **实测偏差见 [converge.md](./converge.md) §七.1**: 平仓/反手分支本就带 `entry.side` 赋值, 实为 3 红(③及两条"平仓后按原方向再开"回归类)4 绿
- [x] T4 落 D1 修复:`context.rs` 两个 `else` 分支各补 `entry.side = req.side;`(附必要注释说明净仓不变式)
- [x] T5 **绿验证**(SC-002 下半):`cargo test -p ricow_strategy` 全绿
- [x] T6 抽取 `position_side_label`(D4):`lua.rs` 净仓快照处替换为纯函数调用 + `mod tests` 新增 FR-005 断言(`(size=0, side=Sell) → "none"`、`(size>0, Buy) → "long"`、`(size>0, Sell) → "short"`)
- [x] T7 门禁全绿(SC-001):`cargo test -p ricow_strategy` / `cargo test -p ricow` / `cargo clippy --all-targets -- -D warnings` / `cargo fmt --all -- --check` 全部 EXIT=0
- [x] T8 回测零变化取证(SC-004/D7):同配置同区间跑 `ricow backtest`, 比对修复前后成交笔数与净盈亏
- [x] T9 真实 Dry Run 复测(SC-003/D8):重建隔离沙箱 `%TEMP%\ricow-e2e`, `e2e_ob` 夹具真实行情下观测 `side` 不再恒单边 / `fills` 买卖交替 / `size` 回落至 `0` 后再次开仓
- [x] T10 文档同步(SC-005):`023-ai-chat-ux/converge.md` §五 F5 标记已修复并指向本变更 + §七 追加对照证据;`specs/lua-api.md` 净仓口径段落核对并补注
- [x] T11 收尾:清理沙箱与实例;本档案补 `converge.md`(含 T3/T5 红绿输出、T8/T9 证据)
