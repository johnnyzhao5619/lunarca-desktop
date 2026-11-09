// 网页版的 ESLint：共用治理底座（`@lunarca/lint-config`，TDD-15 §5），网页只追加独有的两条。
import { governanceConfig, nodeFiles } from "@lunarca/lint-config/eslint";

export default [
  ...governanceConfig,
  { ignores: ["wasm/**", "test-results/**", "playwright-report/**", "e2e/.state/**"] },
  nodeFiles(["scripts/**", "e2e/**", "*.config.ts", "eslint-config-tests.js"]),
  {
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      // 网页是浏览器里的一方客户端：碰不到 Tauri，也不许伸进桌面的代码（TDD-17 §2）。
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            { group: ["@tauri-apps/*"], message: "网页版不碰 Tauri / IPC：要服务端的事经 Worker 的命令表（src/worker/protocol.ts）。" },
            { group: ["**/apps/desktop/**", "**/desktop/src/**"], message: "网页版不依赖桌面应用的代码：两边共用的东西放进 packages/。" },
          ],
        },
      ],
      // HTML 字符串进 DOM 只有一个出口：共享渲染包的 `HostHtml`（经 Trusted Types policy，TDD-17 §6.1）。
      "no-restricted-syntax": [
        "error",
        { selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']", message: "网页里不直接写 innerHTML：HTML 只经共享渲染包的 HostHtml 与 Trusted Types policy。" },
        { selector: "AssignmentExpression[left.property.name=/^(innerHTML|outerHTML)$/]", message: "网页里不直接写 innerHTML / outerHTML（TDD-17 §6.2）。" },
        { selector: "CallExpression[callee.property.name='insertAdjacentHTML']", message: "网页里不直接写 HTML（TDD-17 §6.2）。" },
      ],
    },
  },
];
