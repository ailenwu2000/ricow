# ricow 项目引导

本文件仅为 Hermes 入口指针; 项目原则、纪律、文档职责与变更流程的**唯一权威** = [specs/constitution.md](specs/constitution.md)。

- 文档唯一树: `specs/` — constitution / product / architecture / lua-api / roadmap / research / changes
- 变更流程: 所有变更走 SDD — `/speckit-specify` → `/speckit-plan` → `/speckit-tasks` → `/speckit-implement` → `/speckit-converge`
- 测试纪律: 交易流程必须 testnet 真实调用(币安 demo), 禁 mock 替身、禁假 token; 纯逻辑用单元测试
- 提交纪律: 用户明确说"提交"才可 git commit
