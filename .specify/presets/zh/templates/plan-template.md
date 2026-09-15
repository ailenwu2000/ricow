# 实施计划: [功能]

**分支**: `[###-功能名]` | **日期**: [日期] | **规格**: [链接]

**输入**: `/specs/[###-功能名]/spec.md` 的功能规格

**说明**: 本模板由 `/speckit-plan` 命令填充; 命令定义描述执行流程。

## 摘要

[从功能规格提取: 主要需求 + 技术方案]

## 技术上下文

<!--
  必填: 用本项目具体技术细节替换本节内容。此处结构仅为引导迭代过程。
-->

**语言/版本**: [例如: Rust 1.83, Python 3.11]

**主要依赖**: [例如: mlua, ta, sqlx 或 待澄清]

**存储**: [如适用: SQLite / 文件 / 不适用]

**测试**: [例如: cargo test, pytest 或 待澄清]

**目标平台**: [例如: Linux 服务器, WSL]

**项目类型**: [例如: library/cli/web-service/mobile-app 或 待澄清]

**性能目标**: [领域相关: 例如 1000 req/s, 10k lines/sec, 60 fps 或 待澄清]

**约束**: [领域相关: 例如 <200ms p95, <100MB 内存, 可离线 或 待澄清]

**规模/范围**: [领域相关: 例如 10k 用户, 1M LOC, 50 屏 或 待澄清]

## 宪法检查

*门禁: 必须在 Phase 0 调研前通过; Phase 1 设计后再复查。*

[基于 specs/constitution.md 确定的门禁项]

## 项目结构

### 文档(本次功能)

```text
specs/[###-功能名]/
├── plan.md              # 本文件 (/speckit-plan 命令输出)
├── research.md          # Phase 0 输出 (/speckit-plan 命令)
├── data-model.md        # Phase 1 输出 (/speckit-plan 命令)
├── quickstart.md        # Phase 1 输出 (/speckit-plan 命令)
├── contracts/           # Phase 1 输出 (/speckit-plan 命令)
└── tasks.md             # Phase 2 输出 (/speckit-tasks 命令 — 不由 /speckit-plan 创建)
```

### 源代码(仓库根)

<!--
  必填: 用本功能的实际布局替换下方占位树。删除未用选项, 用真实路径
  展开所选结构。交付的 plan 不得包含 Option 标签。
-->

```text
# [未使用则删除] 选项 1: 单一项目 (默认)
src/
├── models/
├── services/
├── cli/
└── lib/

tests/
├── contract/
├── integration/
└── unit/

# [未使用则删除] 选项 2: Web 应用 (检测到 "frontend" + "backend" 时)
backend/
├── src/
│   ├── models/
│   ├── services/
│   └── api/
└── tests/

frontend/
├── src/
│   ├── components/
│   ├── pages/
│   └── services/
└── tests/
```

**结构决策**: [记录所选结构并引用上方真实目录]

## 复杂度追踪

> **仅当宪法检查出现违规且必须说明时填写**

| 违规项 | 为何需要 | 被否的更简替代方案 |
|--------|----------|--------------------|
| [例如: 第 4 个项目] | [当前需求] | [为何 3 个项目不够] |
| [例如: Repository 模式] | [具体问题] | [为何直接访问 DB 不够] |
