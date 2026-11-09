import tseslint from "typescript-eslint";
import { governanceConfig } from "@lunarca/lint-config/eslint";

export default tseslint.config(
  // 共用治理底座（TDD-15 §5：hooks 结构、禁内联样式、禁写死文案、禁拼接译文），桌面只追加独有的几条。
  ...governanceConfig,
  // tauri-specta 生成物：不手改，故不受 lint 规则约束（改的是生成器或 Rust 侧类型）。
  // cloud-bindings.ts 同源同管线（`cargo run -p luna-cloud --bin export-cloud-bindings`）。
  { ignores: ["src/ipc/bindings.ts", "src/ipc/cloud-bindings.ts"] },
  {
    rules: {
      // 一切 IPC 走生成的 bindings，不手写 `invoke("cmd_name")`（ADR-003 前端零业务、
      // TDD-07 §2）。绕过 bindings 的代价是**类型检查整条失效**：命令名拼错、参数形状
      // 不对、返回类型改了，全都要等到运行时才炸，而 tsc / 单测一路全绿。
      //
      // 只禁具名 `invoke`，不禁整个模块：同一个 `@tauri-apps/api/core` 里的 `Channel`
      // （波形与 ASR partial 走二进制 Channel，ADR-011，5 处合法使用）与 `convertFileSrc`
      // （features/media/media-src.ts）都是**该用的东西**。禁整模块会误伤它们，逼出一排
      // eslint-disable 注释——而一条到处挂着豁免的规则，读的人只会学会怎么再挂一个。
      //
      // 两份生成物（src/ipc/bindings.ts、src/ipc/cloud-bindings.ts）不需要单独豁免：
      // 上面的 `ignores` 已经整体排除它们（生成物不手改）。不在源文件里撒
      // eslint-disable —— 那两个文件由 `export-bindings` / `export-cloud-bindings` 重新
      // 生成，注释会被覆盖掉。这条豁免由 eslint-config-tests.js 断言钉住（既断言两份
      // 生成物确实被忽略，也断言它们确实 import 了 invoke，免得豁免变成空转）。
      // 「去某个地方」只有一扇门（`lib/navigate.ts` 的 `goTo` / `openNote`）。手写
      // `new CustomEvent("luna:navigate", …)` 有两个不会变红的坏处：事件名这个字符串
      // 被抄了 37 遍（打错一个字母，那个按钮从此点了没反应），以及**通道选择漏在每个
      // 调用点手里**——同一个组件被拿到辅助窗里复用时，它的导航会静默失效，因为那扇窗
      // 根本没有监听器。判断收在那扇门里之后，调用方只说去哪儿。
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "NewExpression[callee.name='CustomEvent'][arguments.0.value='luna:navigate']",
          message:
            "导航走 `lib/navigate.ts` 的 `goTo(route)` / `openNote(path)`：它自己按窗口身份选通道（主窗就地派发，辅助窗经 open_workspace_route 唤起主窗）。监听方用 `NAVIGATE_EVENT` 常量。",
        },
      ],
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "@tauri-apps/api/core",
              importNames: ["invoke"],
              message:
                "IPC 走生成的 bindings：用 `commands.<命令名>` / `events.<事件名>`（src/ipc/bindings.ts，云侧 src/ipc/cloud-bindings.ts），改了 Rust 侧就重跑 `cargo run -p luna-app --bin export-bindings`。同模块的 Channel / convertFileSrc 不受此限。",
            },
          ],
        },
      ],
    },
  },
);
