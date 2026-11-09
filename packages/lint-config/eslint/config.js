// 前端 ESLint 治理的共用底座（TDD-15 §5 L1 / L2 / L9 + hooks 结构规则）：桌面、共享包、网页三方
// 都从这里展开，各自只在其上追加自己独有的那几条（桌面的「禁手写 invoke」、包的「不依赖宿主」……）。
// 治理规则只有 `governance.js` 这一份实现；规则本身的反向夹具在 `governance-rule-tests.js`，
// 各宿主的配置有没有真的接上它们，由各自的配置守护另证。
import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import { governanceRules } from "./governance.js";

export const governanceConfig = tseslint.config(
  { ignores: ["dist/**"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    plugins: { luna: { rules: governanceRules }, "react-hooks": reactHooks },
    languageOptions: {
      globals: { ...globals.browser },
    },
    rules: {
      // 条件式 hook 会让整页白屏，而 typecheck 与全部单测都不会红：hook 数量的差异
      // 只在**运行时**跨两次渲染才显形（首帧走早退、hook 不执行；条件变了它才出现，
      // React 当场判「本次 hook 比上次多」）。真机 2026-09-11 就这么把设置页打白了，
      // 而在那之前这个仓库对这一类错误没有任何防线——本规则即那道防线。
      //
      // **只开 `rules-of-hooks`，不开 `exhaustive-deps`**：前者判的是会崩的结构错误，
      // 判据封闭、零误报；后者是启发式，在本仓会一次性刷出大量告警，而「一条到处挂着
      // 豁免的规则，读的人只会学会怎么再挂一个」。
      "react-hooks/rules-of-hooks": "error",
      "luna/no-dom-inline-style": "error",
      "luna/no-user-visible-literal": "error",
      "luna/no-translation-concatenation": "error",
    },
  },
);

/** 跑在 Node 里的文件（构建脚本、端到端用例、工具配置）：用 Node 的全局量，不用浏览器的。 */
export function nodeFiles(files) {
  return { files, languageOptions: { globals: { ...globals.node } } };
}
