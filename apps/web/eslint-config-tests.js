// 网页 ESLint 配置的反向夹具：共用治理规则真的接上了，网页独有的两条（不碰 Tauri / 桌面、不写 innerHTML）
// 真的会红。配置写错时 lint 照样全绿——只有这里会红。
import assert from "node:assert/strict";
import test from "node:test";
import { ESLint } from "eslint";

const eslint = new ESLint({ cwd: import.meta.dirname });

async function ruleIds(code, filePath = "src/features/__fixture__.tsx") {
  const [result] = await eslint.lintText(code, { filePath });
  return result.messages.map((message) => message.ruleId);
}

test("共用治理规则作用于网页：内联样式、写死的界面文案、拼接译文", async () => {
  const ids = await ruleIds(
    'const t = (k) => k;\nexport const A = () => <div style={{ color: "red" }} title={t("a") + "b"}>写死的文案</div>;\n',
  );
  for (const rule of ["luna/no-dom-inline-style", "luna/no-user-visible-literal", "luna/no-translation-concatenation"]) {
    assert.ok(ids.includes(rule), `缺 ${rule}：${ids}`);
  }
});

test("网页不碰 Tauri，也不伸进桌面应用", async () => {
  assert.deepEqual(await ruleIds('import { invoke } from "@tauri-apps/api/core";\nexport const a = invoke;\n'), ["no-restricted-imports"]);
  assert.deepEqual(
    await ruleIds('import { commands } from "../../../desktop/src/ipc/bindings";\nexport const a = commands;\n'),
    ["no-restricted-imports"],
  );
  assert.deepEqual(await ruleIds('import { useState } from "react";\nexport const a = useState;\n'), []);
});

test("网页里不直接写 HTML：dangerouslySetInnerHTML / innerHTML / insertAdjacentHTML", async () => {
  assert.deepEqual(await ruleIds("export const A = ({ h }) => <div dangerouslySetInnerHTML={{ __html: h }} />;\n"), [
    "no-restricted-syntax",
  ]);
  assert.deepEqual(await ruleIds("export function f(e, h) { e.innerHTML = h; }\n", "src/lib/__fixture__.ts"), ["no-restricted-syntax"]);
  assert.deepEqual(await ruleIds('export function f(e, h) { e.insertAdjacentHTML("beforeend", h); }\n', "src/lib/__fixture__.ts"), [
    "no-restricted-syntax",
  ]);
});
