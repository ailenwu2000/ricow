# 022 任务分解

- [ ] T1 `cargo fmt --all`, 记录改动文件数与行数
- [ ] T2 验证: fmt --check 零差异 / clippy -D warnings 零输出 / 测试 339-0-12 / release 构建通过
- [ ] T3 单独提交(提交信息说明: 纯格式, 无语义改动)
- [ ] T4 `ci.yml` 打开 `cargo fmt --all -- --check`
- [ ] T5 CONTRIBUTING 与 roadmap 同步(格式由 CI 强制; 关闭待办项)
- [ ] T6 推送后确认 CI 三作业全绿; 补 converge.md
