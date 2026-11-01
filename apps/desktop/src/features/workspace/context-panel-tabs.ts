/**
 * 「上下文」面板的页签注册表（PRD-07-031 / PRD-07-063）。
 *
 * 与 React 组件分离，既让守护测试可直接消费，也避免组件模块导出非组件值导致
 * Fast Refresh 退化为整页重载（同 `graph-sidebar-config.ts` 的理由）。
 */

export type ContextTab =
  | "outline"
  | "backlinks"
  | "outgoing"
  | "related"
  | "tags"
  | "info"
  | "history";

/**
 * 页签集与呈现序。
 *
 * 分档：**结构**（大纲）→ **关系**（反链 / 外链 / 相关）→ **这一篇是什么**（标签 / 信息）
 * → **这一篇的过去**（版本）。标签与信息并排，因为它们同属当前对象的事实（PRD-07-031）；
 * 版本历史另起一档，它谈的是时间轴而不是当下。
 */
export const CONTEXT_TABS = [
  { id: "outline", label: "panel.outline" },
  { id: "backlinks", label: "panel.backlinks" },
  { id: "outgoing", label: "panel.outgoing" },
  { id: "related", label: "panel.related" },
  { id: "tags", label: "panel.noteTags" },
  { id: "info", label: "panel.noteInfo" },
  { id: "history", label: "panel.version" },
] as const satisfies ReadonlyArray<{ id: ContextTab; label: string }>;
