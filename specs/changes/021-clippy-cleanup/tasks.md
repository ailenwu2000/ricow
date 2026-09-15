# 021 任务分解

- [ ] T1 `cargo clippy --workspace --all-targets --locked --fix`,逐文件复核 diff
- [ ] T2 手工:`too_many_arguments` ×4 → `#[allow]` + 注释(注明"参数聚合单独立项")
- [ ] T3 手工:`await_holding_lock` ×2(测试 ENV_LOCK)→ `#[allow]` + 注释
- [ ] T4 手工:`backtest.rs` 复杂类型 → 局部 `type` 别名
- [ ] T5 验证:`cargo clippy ... -- -D warnings` 零输出;`cargo test --workspace` 339/0/12
- [ ] T6 `ci.yml` lint 作业启用 `-D warnings`;推送后 CI 绿
- [ ] T7 反向验证门禁:临时引入一处告警 → CI 红 → 撤销(留记录)
- [ ] T8 收尾:specs/architecture.md 测试基线数字同步;本档案补 converge.md
