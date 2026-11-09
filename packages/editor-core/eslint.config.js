// 共用 ESLint 治理底座（TDD-15 §5：禁内联样式、禁写死用户可见文案、禁拼接翻译、hooks 规则……）
// 来自 `@lunarca/lint-config`，桌面、本包与网页同一份。
// 包在此之上多一条：渲染器与编辑器不依赖 IPC、不依赖任何宿主（TDD-17 §2）——宿主能力只经端口进来
// （`src/host.ts` 的 `MarkdownHost`、`src/reading/reading-vault.ts`、`src/editor/editor-host.ts` 的 `EditorHost`）。
import { governanceConfig } from "@lunarca/lint-config/eslint";

export default [
  ...governanceConfig,
  {
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["@tauri-apps/*"],
              message: "共享渲染包不碰 Tauri / IPC：要宿主做的事经端口（`MarkdownHost` / `ReadingVault` / `EditorHost`）由宿主注入。",
            },
            {
              group: ["**/apps/**", "**/ipc/**"],
              message: "共享渲染包不得反向依赖宿主应用的代码：要宿主做的事经端口（`MarkdownHost` / `ReadingVault` / `EditorHost`）注入。",
            },
          ],
        },
      ],
    },
  },
];
