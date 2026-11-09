# 贡献指南

感谢你愿意为 Lunarca 出力。本文只写「怎么参与」；环境准备与构建见 [README](README.md)「快速开始」。

## 开工前先读

1. [README](README.md)「架构原则」——单一 AI 主干、前端零业务、笔记即文件、隐私优先。**违反其中任一条会导致架构性返工**，请务必先读。
2. 编码、命名、目录、样式、i18n 与 lint 的规则由下文「提交前必须自检」的门禁命令机器执行，提交前在本地跑一遍即可。

产品需求与技术设计文档（`PRD-…` / `TDD-…` / `ADR-…`）不随本仓分发；代码注释与提交信息里会以编号出现。

## 环境

```bash
brew install meson ninja cmake  # macOS：AEC3 与 libopus 的随包 C/C++ 构建链
npm ci                          # 前端依赖（npm workspaces，在仓库根装，唯一 lockfile 在根）
node scripts/dev.mjs            # 启动开发环境（自动躲开被占端口）
```

Rust 版本由 `rust-toolchain.toml` 钉死，Node 版本由 `.nvmrc` 钉死——**不要绕过它们**，环境漂移的代价由所有人分摊。

## 提交前必须自检

维护者合入前会跑同一套门禁，本地先跑一遍能省一个来回：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix apps/desktop run typecheck
npm --prefix apps/desktop run lint
npm --prefix apps/desktop run lint:css
npm --prefix apps/desktop run build
npm --prefix packages/editor-core run lint
npm --prefix packages/editor-core run typecheck
npm --prefix packages/editor-core run test
npm --prefix packages/lint-config run lint
npm --prefix packages/lint-config run typecheck
```

网页与共享包的改动还需按 [README](README.md#快速开始) 准备 WASM 工具链并生成绑定，再执行以下命令。

```bash
cargo test -p luna-web --target wasm32-unknown-unknown
npm --prefix apps/web run lint
npm --prefix apps/web run lint:css
npm --prefix apps/web run typecheck
npm --prefix apps/web run test
npm --prefix apps/web run build
```

改动了 Rust 侧的 command / event 类型，还要重新生成 TS 绑定，否则 CI 的无漂移检查会红：

```bash
cargo run -p luna-app --bin export-bindings
```

## 提交规范

- 遵循 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/)：`feat:` / `fix:` / `docs:` / `refactor:` / `ci:` / `test:` / `chore:`。
- 提交信息用中文正文说清**为什么**这么改，而不只是改了什么——「读一遍 diff 就知道」的内容不必写进提交信息。
- 改动须能回溯到对应的需求编号（`PRD-<模块号>-<序号>`）或技术决策编号（`ADR-<序号>`）。

## 改文档的规矩

- **DRY**：某个决策只在它的归属文档里修改，其他文档**指向它、不复制**。发现复制即为待收口的债。
- **先改规格再改实现**：规格变了先改 PRD/TDD，路线图只更新阶段状态。
- **假设要标注**：需求未明确、由你判断的地方，统一写 `> 假设：…（理由：…）`。
- 文档用中文；代码标识符、命令、crate 名、Token 与 i18n key 用英文。

## 关于「验证」的一条硬约定

**不要把 headless 全绿写成已验收。** 需要真实硬件、系统权限、本地模型或跨启动行为才能确认的结论，只有在真机上跑过才算数；没跑过就在 PR 里明写「未验」，说明为什么没跑。这条在本仓库被违反过多次，代价一律是返工。

## Pull Request

公开仓 [johnnyzhao5619/lunarca-desktop](https://github.com/johnnyzhao5619/lunarca-desktop) 的每个版本都是从开发仓生成的源码快照，CLA 签署机制也还没上线，所以**暂不接受 Pull Request**。想改什么，先开 Issue 讨论；需要代码时维护者会在开发仓里实现，并在 Issue 里注明对应的版本。

## 报告问题

- 缺陷与需求走 [GitHub Issues](https://github.com/johnnyzhao5619/lunarca-desktop/issues)。公开仓每个发布版本一个提交、打同名标签 `v<版本>`，开发不在上面进行。
- **安全问题不要开公开 Issue**，见 [SECURITY.md](SECURITY.md)。

## 许可证与 CLA

- 本仓库以 [AGPL v3](LICENSE) 发行；版权归属见 [COPYRIGHT](COPYRIGHT)，开源版与商业版的边界承诺见 [LICENSE_FAQ.md](LICENSE_FAQ.md)。
- 项目计划以「AGPL 开源版 + 闭源商业版」双许可发行（ADR-029、TDD-12 §7）。为保持双许可的法律有效性，**外部代码贡献须先签署 CLA**（你保留版权，授予项目方再许可权，草案见 [docs/legal/CLA.md](docs/legal/CLA.md)）。**CLA 签署机制上线前，暂不合并外部代码 PR**——issue、讨论与文档反馈一如既往欢迎。
- 任何新增依赖、模型或字体都必须过 License 核查（`cargo deny check`），与 AGPL 不兼容或许可未知的依赖会被 CI 阻断。
