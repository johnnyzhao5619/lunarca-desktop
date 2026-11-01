/**
 * 笔记类型（`luna.type`）→ 人话译名的**唯一** i18n key 口径。
 *
 * 两个消费者：关系图谱的类型勾选组（`features/graph/graph-sidebar-config.ts`）与右侧
 * 上下文面板的「笔记信息」。各写一份五个词的译名，会在改名那天分叉——而分叉的表现是
 * 同一篇笔记在两处叫两个名字，没有任何东西会因此变红。
 *
 * **住在 `lib/` 而不是任一 feature 里**：两个消费者分属 `workspace` 与 `graph` 两个
 * 功能模块，把它留在其中一边，另一边就得跨 feature 引用——那正是结构门禁拦下的
 * 「功能模块存在循环依赖」（本批第一版就是这么被拦住的）。
 *
 * 译名住在中性的 `workspace:noteType.*`，键即 wire 上的 kebab 值。
 */
import type { NoteTypeDto } from "../ipc/bindings";

/**
 * 全部笔记类型（与 `NoteTypeDto` 同集）。
 *
 * `satisfies` 保证它既是**具体的元组**（守护测试可逐项遍历），又被编译器核对为
 * `NoteTypeDto` 的合法取值——后端加一种类型时，这里漏了不会报错，故另有守护测试按
 * 图谱注册表逐条比对（`graph-rescan-i18n.test.ts`）。
 */
export const NOTE_TYPES = [
  "note",
  "quick-note",
  "recording",
  "transcript",
  "chat",
  "document",
  "video-project",
  "script",
  "swipe",
] as const satisfies ReadonlyArray<NoteTypeDto>;

/** 该类型的译名 i18n key（`workspace` 命名空间内）。 */
export function noteTypeLabelKey(noteType: NoteTypeDto): string {
  return `noteType.${noteType}`;
}
