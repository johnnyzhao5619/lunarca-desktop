/**
 * 一方应用的详情页深链（ADR-169 ⑧）。放在 `lib/` 而不是 `features/apps/`：各个功能模块都要能链到
 * 某个应用的详情页（缺组件时「去『从链接导入』处理」），而「应用」页本身又引用这些功能模块的面板——
 * 写在 `features/apps/` 里就成了循环依赖。`AppId` 由 Rust 导出，写错应用 id 过不了编译。
 */

import type { AppId } from "../ipc/bindings";

export function appDetailRoute(id: AppId): string {
  return `apps://${id}`;
}

/** 「应用 › 模板」页签（纵切 7b）：各选择面底部的「获取更多模板」链到这里（PRD-18-004）。同上，放 `lib/` 免得
 * 编辑器、整理与新建笔记的模块反过来依赖「应用」页。与 `features/apps/apps-route.ts` 的 `appsTabRoute("templates")` 同值。 */
export const APPS_TEMPLATES_ROUTE = "apps://templates";
