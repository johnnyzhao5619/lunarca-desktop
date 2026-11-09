/**
 * Trusted Types（TDD-17 §6.1）：CSP 带 `require-trusted-types-for 'script'; trusted-types lunarca`，
 * 页面里只有这一个名为 `lunarca` 的 policy。它只放行两样：
 * - HTML：共享渲染包 `HostHtml` 的唯一出口（KaTeX `trust: false` 与 Mermaid strict 模式的输出）；
 * - 脚本地址：只认本站同源的地址（专用 Worker 的入口）。
 * 浏览器不支持 Trusted Types 时原样放过（CSP 的其余条款照样生效）。
 */

interface LunarcaPolicy {
  createHTML(input: string): TrustedHTML;
  createScriptURL(input: string): string;
}

interface PolicyFactory {
  createPolicy(
    name: string,
    rules: { createHTML(input: string): string; createScriptURL(input: string): string },
  ): LunarcaPolicy;
}

export const POLICY_NAME = "lunarca";

let policy: LunarcaPolicy | null = null;

export function installTrustedTypesPolicy(): void {
  if (policy) return;
  const factory = (globalThis as { trustedTypes?: PolicyFactory }).trustedTypes;
  if (!factory) return;
  policy = factory.createPolicy(POLICY_NAME, {
    createHTML: (input) => input,
    createScriptURL: (input) => {
      const url = new URL(input, window.location.href);
      if (url.origin !== window.location.origin) throw new TypeError("script_url_rejected");
      return url.href;
    },
  });
}

export function trustedHtml(html: string): string | TrustedHTML {
  return policy ? policy.createHTML(html) : html;
}

export function trustedScriptUrl(url: string): string {
  return policy ? policy.createScriptURL(url) : url;
}
