/**
 * 包内文案（react-i18next）：包自带两种语言的资源，宿主把它们并进自己的 i18next 实例，
 * 命名空间固定为 `editor-core`。组件里写 `useTranslation("editor-core")` 字面量——静态 key 门禁
 * 按字面量认命名空间。中英键集对齐由宿主的 locale 门禁一并覆盖。
 */

import enUS from "./locales/en-US/editor-core.json";
import zhCN from "./locales/zh-CN/editor-core.json";

export const EDITOR_CORE_NAMESPACE = "editor-core";

export const EDITOR_CORE_LOCALES = {
  "zh-CN": zhCN,
  "en-US": enUS,
} as const;
