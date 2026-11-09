# Lunarca

**数据留在本地、算力可选的桌面 AI 工作站。**

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL%20v3-blue.svg)](LICENSE)

把翻译、语音输入、会议录音转写、笔记整合进单一工作区，所有数据落到本地 Markdown Vault。AGPL v3 开源。

未连接服务端时不产生云网络行为。连接后使用所选服务端的算力，麦克风上云需单独同意，云同步按库开启。联网行为可见、可关闭，严格本地模式可阻止出站联网。凭据存入本机加密保险箱，保险箱主密钥保存在系统钥匙串。详细数据边界见 [PRIVACY.md](PRIVACY.md)。

平台：**macOS 优先**（Apple Silicon），Windows 并行兼容但未做同等真机验收。

## 快速开始

```bash
# 前置：Rust 1.96（rust-toolchain.toml 已钉死）、Node 22（.nvmrc）
# macOS 还需 meson + ninja（AEC3 回声消除以 bundled C++ 构建）与 cmake（实时转写上行的
# Opus 编码随包构建 libopus）
brew install meson ninja cmake
# 随包资源：内置转写模型与本机推理引擎 llama-server（LLM/嵌入，ADR-143），校验后放进 src-tauri/resources
python3 scripts/fetch-bundled-asr.py
python3 scripts/fetch-llama-runtime.py
# 前端依赖：npm workspaces，在仓库根装（唯一 lockfile 在根）
npm ci

# 开发
node scripts/dev.mjs       # 推荐：自动躲开被占用的端口
npx tauri dev              # 使用仓库钉住的 CLI，直连固定 1420 端口

# 测试
cargo test --workspace                   # 全部 Rust 单测/集成测试
npm --prefix apps/desktop run test        # 前端 Vitest
```

日常检查见 [贡献指南](CONTRIBUTING.md#提交前必须自检)。

网页版的 WASM 构建还需要 `wasm32-unknown-unknown` 目标，以及与 `Cargo.lock` 同版本的 `wasm-bindgen-cli`。
在仓库根目录安装依赖后执行以下命令。

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked --version <Cargo.lock 中 wasm-bindgen 的版本>
npm --prefix apps/web run build:wasm
npm --prefix apps/web run test
npm --prefix apps/web run build
```

## 技术栈

| 层 | 技术 | 说明 |
|---|---|---|
| 桌面壳 | **Tauri 2**（Rust, macOS 优先 + Windows 兼容） | 无 Electron 的电量/内存开销 |
| 后端 | **Rust** workspace（`crates/luna-*` 库 crate + 应用 crate） | 业务逻辑全在 Rust，前端零业务 |
| 前端 | **React + TypeScript + CodeMirror 6** | Vite + Vitest |
| AI 引擎 | **whisper-rs**（ASR，进程内 FFI）+ 随包 **llama-server**（LLM/嵌入，进程外 HTTP/SSE） | 引擎分治，不统一 sidecar |
| 数据 | **本地 Markdown Vault**（笔记即文件） | SQLite 仅派生索引，无私有格式 |

## 项目结构

```
Lunarca/
├── src-tauri/           # Tauri 桌面应用装配（Rust，crate 名 luna-app）
│   └── tauri.conf.json
├── apps/desktop/        # 桌面 React 前端
├── apps/web/            # 网页版笔记（ADR-177）
├── packages/editor-core/ # 桌面与网页共用的编辑器与渲染（宿主能力经端口注入）
├── packages/styles/     # 共用设计 Token 与编辑面 / 阅读面样式
├── packages/lint-config/ # 共用 ESLint 治理与 CSS 守护判据
├── crates/
│   ├── luna-core/       # 核心数据类型（零 IO，不依赖任何上层）
│   ├── luna-config/     # 配置注册表 + 设置管理 + 本机密钥保险箱
│   ├── luna-engine/     # 本地推理引擎（llama-server 监管 + whisper ASR）
│   ├── luna-provider/   # AI Provider 抽象层（ASR/LLM/嵌入）
│   ├── luna-jobqueue/   # Job 队列调度
│   ├── luna-audio/      # 音频采集管线（VAD/重采样/AEC3）
│   ├── luna-fetch/      # 网络媒体抓取
│   ├── luna-docimport/  # 文档导入（txt/md/CSV/HTML/docx → Markdown）
│   ├── luna-vault/      # Markdown Vault（索引/解析/持久化/回收站）
│   ├── luna-export/     # 导出管线
│   ├── luna-metrics/    # 本地知识闭环度量
│   ├── luna-model-catalog/ # 本地模型目录
│   ├── luna-model-fetch/   # 模型下载与校验
│   ├── luna-content-schema/ # 主题包与模板包的公共契约
│   ├── luna-content/    # 主题与模板的存储与安装
│   ├── luna-cloud/      # 账户、凭据与云端接入
│   ├── luna-opaque/     # 口令登录（OPAQUE）的密码学与报文格式
│   ├── luna-sync/       # 云端同步的客户端核心
│   ├── luna-sync-core/  # 同步与合并共用的纯逻辑（桌面与网页同一份）
│   ├── luna-web/        # 网页版笔记的 WASM 封装
│   └── luna-cli/        # CLI 工具（luna translate 等）
├── docs/legal/          # 贡献者许可协议（CLA）草案
└── scripts/             # 构建、发布、门禁与开发脚本
```

## 架构原则

- **单一 AI 主干**：一切 AI 动作走 `AIProvider + JobQueue + ResultSchema`
- **前端零业务**：Rust 做全部业务，前端只发命令 + 订阅事件 + 渲染
- **笔记即文件**：Markdown Vault 是唯一事实源，任何编辑器可打开
- **隐私优先**：联网可见可关，日志脱敏，凭据加密存入本机保险箱

## 文档

| 想知道 | 看 |
|---|---|
| 这个版本改了什么 | [CHANGELOG.md](CHANGELOG.md) |
| 怎么参与贡献 | [CONTRIBUTING.md](CONTRIBUTING.md) |
| 报告安全问题 | [SECURITY.md](SECURITY.md) |
| 本地存储、联网与同步的数据边界 | [PRIVACY.md](PRIVACY.md) |

## 已知限制

- 应用**不经 Apple 公证**（不加入 Apple Developer Program）：官方安装包用发布者的固定自签证书签名，首次打开需在「系统设置 → 隐私与安全性」点「仍要打开」；之后的应用内更新经发布者签名校验，不再需要放行。安装包在 [lunarca.app](https://lunarca.app)「开始使用」页下载，页上有校验值与放行步骤。
- Windows 版暂不发布：源码保持跨平台、能在 Windows 上构建运行，但未做与 macOS 同等的真机验收，官方暂不提供 Windows 安装包。
- 转写与库内问答的质量取决于本地模型规格，小模型在长会议与嘈杂环境下会明显退化。

## 许可证

AGPL v3，见 [LICENSE](LICENSE)。每个安装包版本的完整对应源码公开在 [johnnyzhao5619/lunarca-desktop](https://github.com/johnnyzhao5619/lunarca-desktop) 的同名标签（`v<版本>`）下。版权归属见 [COPYRIGHT](COPYRIGHT)；开源版与商业版的边界承诺见 [LICENSE_FAQ.md](LICENSE_FAQ.md)；商标使用见 [TRADEMARK.md](TRADEMARK.md)。
