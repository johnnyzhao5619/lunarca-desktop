/**
 * 关系图谱侧栏的静态呈现注册表。
 *
 * 与 React 组件分离，既让动态 i18n 守护可直接消费，也避免组件模块导出非组件值导致
 * Fast Refresh 退化为整页重载。
 */

/** 笔记类型勾选组（ADR-024）：与 `GRAPH_NOTE_TYPES` 同序。
 *
 * **`label` 是完整 i18n key，且落在中性的 `noteType.*` 而不是 `graph.*`**：
 * 「这一类笔记叫什么」不是图谱的事实，右侧上下文面板的「笔记信息」也要说同一句话
 * （[`noteTypeLabelKey`](../../lib/note-type.ts)）。留在 `graph.` 下的话，第二个消费者
 * 要么写 `t("graph.typeQuickNote")`（在一个与图谱无关的面板里），要么另抄一份五个词的
 * 译名——而抄出来的那份会在改名那天与这一份分叉。
 */
export const NOTE_TYPE_FILTERS = [
  { type: "note", label: "noteType.note", group: "knowledge" },
  { type: "quick-note", label: "noteType.quick-note", group: "knowledge" },
  { type: "recording", label: "noteType.recording", group: "material" },
  { type: "transcript", label: "noteType.transcript", group: "material" },
  { type: "chat", label: "noteType.chat", group: "material" },
  { type: "document", label: "noteType.document", group: "material" },
  { type: "video-project", label: "noteType.video-project", group: "knowledge" },
  { type: "script", label: "noteType.script", group: "knowledge" },
  { type: "swipe", label: "noteType.swipe", group: "knowledge" },
] as const;

/**
 * 「记录」开关挂在哪个小组下（PRD-06-052 / ADR-086）。
 *
 * **它刻意不进 `NOTE_TYPE_FILTERS`**：成熟度是与来源**正交**的第二根轴，而那份清单
 * 逐条对应 `GRAPH_NOTE_TYPES`（守护测试按序逐一比对）。把它塞进去会立刻教会用户一句
 * 错话——「记录是第六种笔记类型」——而事实是一篇记录既可以是 `note` 也可以是速记。
 * 故它在版式上另起一行、带自己的「成熟度」小标题、**不带类型色点**（图上没有"记录色"：
 * 记录节点仍按它自己的类型着色），三处线索一起说同一件事。
 */
export const MATURITY_FILTER_GROUP = "knowledge";

/** 两小组（PRD-06-048③）：把同一批类型开关呈现为「知识 / 资料」。 */
export const NOTE_TYPE_GROUPS = [
  { id: "knowledge", label: "groupKnowledge" },
  { id: "material", label: "groupMaterial" },
] as const;
