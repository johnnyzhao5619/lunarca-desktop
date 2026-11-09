// 反向夹具：包的 ESLint 配置真的在管包的源码（治理规则沿用 `@lunarca/lint-config` 那一份，另加「不依赖宿主」一条）。
// 配置写错（漏 `files`、规则被后面的对象盖掉、忘了展开共用底座）时 lint 照样全绿——只有这里会红。
import assert from "node:assert/strict";
import test from "node:test";
import { ESLint } from "eslint";

const eslint = new ESLint({ cwd: import.meta.dirname });

async function ruleIds(code, filePath = "src/render/__fixture__.tsx") {
  const [result] = await eslint.lintText(code, { filePath });
  return result.messages.map((message) => message.ruleId);
}

test("包的源码不得碰 Tauri / IPC，也不得相对路径伸进宿主应用", async () => {
  assert.deepEqual(await ruleIds('import { invoke } from "@tauri-apps/api/core";\nexport const a = invoke;\n'), [
    "no-restricted-imports",
  ]);
  assert.deepEqual(
    await ruleIds('import { commands } from "../../../../apps/desktop/src/ipc/bindings";\nexport const a = commands;\n'),
    ["no-restricted-imports"],
  );
  assert.deepEqual(await ruleIds('import { useState } from "react";\nexport const a = useState;\n'), []);
});

test("共用治理规则同样作用于包：内联样式、写死的界面文案", async () => {
  const ids = await ruleIds('export const A = () => <div style={{ color: "red" }}>写死的文案</div>;\n');
  assert.ok(ids.includes("luna/no-dom-inline-style"), `缺 no-dom-inline-style：${ids}`);
  assert.ok(ids.includes("luna/no-user-visible-literal"), `缺 no-user-visible-literal：${ids}`);
});
