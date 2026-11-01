/**
 * 界面文案不得含 Markdown 强调标记。
 *
 * 资源串一律按**纯文本**渲染（`<p>{t(key)}</p>` 这类）。写 `**默认关**` 的结果是界面上
 * 原样出现两对星号——写的人在编辑器里看着是加粗，用户看到的是标点噪声。
 *
 * 覆盖**全部** locale 资源包而不只是设置页：此前只守 `settings.json`，于是首启引导的
 * 「组织服务端」说明带着星号一路进了发布版（PRD-14-005 那条路的正文）。
 *
 * 要强调就用词本身（「默认关闭」「绝不」「必须」），中文还有「」可用。
 */

import { describe, expect, it } from "vitest";

const BUNDLES = import.meta.glob("./locales/*/*.json", { eager: true }) as Record<
  string,
  { default: unknown }
>;

/** 递归收集所有叶子字符串及其点分路径。 */
function leaves(node: unknown, path: string[] = []): Array<[string, string]> {
  if (typeof node === "string") return [[path.join("."), node]];
  if (node && typeof node === "object") {
    return Object.entries(node).flatMap(([key, value]) => leaves(value, [...path, key]));
  }
  return [];
}

describe("界面文案是纯文本", () => {
  it("资源包不是空的（glob 失效时本守护会假绿）", () => {
    expect(Object.keys(BUNDLES).length).toBeGreaterThan(0);
  });

  for (const [path, module] of Object.entries(BUNDLES)) {
    it(`${path} 不含 Markdown 强调标记`, () => {
      const offenders = leaves(module.default)
        .filter(([, text]) => /\*\*|__[^_]/.test(text))
        .map(([key, text]) => `${key}: ${text.slice(0, 60)}…`);
      expect(offenders, "这些文案会把星号原样显示给用户").toEqual([]);
    });
  }
});
