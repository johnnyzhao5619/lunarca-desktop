// 三条自定义治理规则的反向夹具（TDD-15 §5 L1 / L2 / L9）：判据写错时 lint 照样全绿，只有这里会红。
// 各宿主（桌面、共享包、网页）的 ESLint 配置有没有真的接上这些规则，由各自的配置守护另证。
import { RuleTester } from "eslint";
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
