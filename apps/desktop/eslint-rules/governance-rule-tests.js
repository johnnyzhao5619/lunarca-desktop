import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
import test from "node:test";
import { ESLint, RuleTester } from "eslint";
import { governanceRules } from "./governance.js";

const tester = new RuleTester({
  languageOptions: {
    ecmaVersion: 2022,
    sourceType: "module",
    parserOptions: { ecmaFeatures: { jsx: true } },
  },
});

tester.run("luna/no-dom-inline-style", governanceRules["no-dom-inline-style"], {
  valid: [
    { code: 'const view = <div style={cssVariables({ "--progress": value })} />;' },
    { code: "const view = <div style={ready ? cssVariables(vars) : undefined} />;" },
    { code: "const view = <Control style={control.style} />;" },
  ],
  invalid: [
    { code: "const view = <div style={{ color: value }} />;", errors: [{ messageId: "inlineStyle" }] },
    { code: "const view = <div style={runtimeStyle} />;", errors: [{ messageId: "inlineStyle" }] },
    { code: "const view = <div style={makeStyle()} />;", errors: [{ messageId: "inlineStyle" }] },
  ],
});

tester.run("luna/no-user-visible-literal", governanceRules["no-user-visible-literal"], {
  valid: [
    { code: "const view = <p>{t('message')}</p>;" },
    { code: "const view = <div>{count}</div>;" },
    { code: "const view = <div>{state.kind === 'ready' && <p>{t('ready')}</p>}</div>;" },
    { code: "const view = <span>{' · '}</span>;" },
  ],
  invalid: [
    { code: "const view = <p>Hello</p>;", errors: [{ messageId: "literal" }] },
    { code: "const view = <p>{'你好'}</p>;", errors: [{ messageId: "literal" }] },
    { code: "const view = <p>{'Hello ' + name}</p>;", errors: [{ messageId: "literal" }] },
    { code: "const view = <button aria-label={ready ? 'Open' : 'Close'} />;", errors: [{ messageId: "literal" }] },
    { code: "const view = <img alt={`Preview ${name}`} />;", errors: [{ messageId: "literal" }] },
  ],
});

tester.run(
  "luna/no-translation-concatenation",
  governanceRules["no-translation-concatenation"],
  {
    valid: [
      { code: "const message = t('greeting', { name });" },
      { code: "const message = plain + value;" },
    ],
    invalid: [
      { code: "const message = t('prefix') + name;", errors: [{ messageId: "concatenation" }] },
      { code: "const message = `${tWorkspace('status')} ${count}`;", errors: [{ messageId: "concatenation" }] },
      { code: "const message = i18n.t('prefix') + name;", errors: [{ messageId: "concatenation" }] },
      { code: "const message = String(t('prefix')) + name;", errors: [{ messageId: "concatenation" }] },
    ],
  },
);

/**
 * **反向夹具：禁手写 `invoke`**（eslint.config.js 的 `no-restricted-imports`）。
 *
 * 上面三条是自定义规则，用 RuleTester 直接喂规则实现即可。这一条是**内置规则 + 项目配置**
 * ——真正要证明的不是「no-restricted-imports 这个规则能工作」（那是 ESLint 自己的事），
 * 而是「**本仓这份 eslint.config.js** 会因为手写 invoke 变红、且不会误伤 Channel /
 * convertFileSrc / 两份生成物」。所以这里用 ESLint 的 Node API 跑**真实配置**，
 * 而不是 RuleTester 跑一份重新拼出来的配置——后者证明的是那份重拼的配置，不是仓库的。
 *
 * 一条从未被证明会红的规则等于没有：写下它时全仓实测零违规，若判据写错（比如误写成
 * 禁整个模块、或 importNames 拼错），lint 一样全绿，没有任何东西会告诉你它是空的。
 */
test("no-restricted-imports 挡住手写 invoke，且不误伤 Channel / convertFileSrc / 生成物", async () => {
  const eslint = new ESLint();
  const restricted = async (code) => {
    const [result] = await eslint.lintText(code, { filePath: "src/features/__fixture__.ts" });
    return result.messages.filter((m) => m.ruleId === "no-restricted-imports");
  };

  // ① 手写 invoke —— 必须红。
  const handwritten = await restricted(
    'import { invoke } from "@tauri-apps/api/core";\nexport const run = () => invoke("cmd_ping");\n',
  );
  assert.equal(handwritten.length, 1, "手写 invoke 必须被 no-restricted-imports 挡下");
  assert.match(handwritten[0].message, /commands\./);

  // ② 换个别名照样红（`invoke as x` 走的是 importNames，不是本地名字）。
  assert.equal(
    (await restricted('import { invoke as call } from "@tauri-apps/api/core";\ncall("cmd_ping");\n'))
      .length,
    1,
    "改个别名不该能绕过",
  );

  // ③ 同一个模块里的合法用法一个都不许误伤。
  for (const code of [
    'import { Channel } from "@tauri-apps/api/core";\nexport const c = new Channel();\n',
    'import { convertFileSrc } from "@tauri-apps/api/core";\nexport const s = convertFileSrc("/a");\n',
    'import { Channel, convertFileSrc } from "@tauri-apps/api/core";\nexport const x = [Channel, convertFileSrc];\n',
    // 局部变量/参数叫 invoke 与这条规则无关（use-chat-session.ts 就有一个）。
    "export const run = (invoke) => invoke(1);\n",
  ]) {
    assert.deepEqual(await restricted(code), [], `合法用法被误伤了：\n${code}`);
  }

  // ④ 豁免只给两份生成物，且这条豁免必须是**有用的**：两个文件确实 import 了 invoke
  //    （否则「豁免」只是句空话），而 ESLint 确实不检查它们（顶部 ignores 已整体排除）。
  for (const generated of ["src/ipc/bindings.ts", "src/ipc/cloud-bindings.ts"]) {
    assert.match(
      readFileSync(generated, "utf8"),
      /import\s*\{[^}]*\binvoke\b[^}]*\}\s*from\s*"@tauri-apps\/api\/core"/,
      `${generated} 不再 import invoke —— 这条豁免已经没有对象，删掉它`,
    );
    assert.equal(await eslint.isPathIgnored(generated), true, `${generated} 应被 ESLint 忽略`);
  }

  // ⑤ 豁免是**逐文件**的，不是给整个 src/ipc/ 开的口子。
  assert.equal(await eslint.isPathIgnored("src/ipc/handwritten.ts"), false);
  assert.equal(
    (
      await eslint.lintText('import { invoke } from "@tauri-apps/api/core";\ninvoke("x");\n', {
        filePath: "src/ipc/handwritten.ts",
      })
    )[0].messages.filter((m) => m.ruleId === "no-restricted-imports").length,
    1,
    "src/ipc/ 下的手写文件同样要被挡下",
  );
});
