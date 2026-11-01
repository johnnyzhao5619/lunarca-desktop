/// <reference types="vite/client" />
/**
 * DOM 用例共用的 i18n 实例：**装的是真资源包**，不是 mock 字典。
 *
 * 换成 `t: (key) => key` 之类的桩，"文案取自 i18n key"这条断言就退化成同义反复——key
 * 写错、资源包里压根没这条目，用例照样绿。装真包之后，缺 key 时 i18next 回落成裸 key
 * 字符串，与期望的译文不等，用例即红。
 *
 * 语言钉死 `zh-CN`：应用入口按 `navigator.language` 选语言，跟着它走会让同一份用例在
 * 不同机器上比对不同文案。
 *
 * 命名空间由 glob 派生而不是逐条 import——手抄一份清单，`i18n.ts` 每加一个命名空间这里
 * 就旧一次，而"少一个命名空间"的表现是用例里冒出裸 key，看起来像组件坏了。
 */

import { createElement, type ReactElement } from "react";
import { createInstance, type i18n as I18nInstance } from "i18next";
import { I18nextProvider } from "react-i18next";

const BUNDLES = import.meta.glob("./locales/zh-CN/*.json", { eager: true }) as Record<
  string,
  { default: Record<string, unknown> }
>;

const resources: Record<string, Record<string, unknown>> = {};
for (const [path, module] of Object.entries(BUNDLES)) {
  const namespace = path.slice(path.lastIndexOf("/") + 1, -".json".length);
  resources[namespace] = module.default;
}

/** 测试用 i18n 实例（zh-CN，全部命名空间）。 */
export const testI18n: I18nInstance = createInstance();
void testI18n.init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  defaultNS: "common",
  ns: Object.keys(resources),
  resources: { "zh-CN": resources },
  interpolation: { escapeValue: false },
});

/** 按 key 取译文——**用与渲染同一个实例**，用例因此不硬写中文字面量。 */
export function t(key: string, options?: Record<string, unknown>): string {
  return testI18n.t(key, options);
}

/** 给待测组件包上 i18n 宿主。 */
export function withI18n(element: ReactElement): ReactElement {
  return createElement(I18nextProvider, { i18n: testI18n }, element);
}
