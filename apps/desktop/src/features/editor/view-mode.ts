// 三态视图的切换规则（PRD-06-017：Live Preview / 源码 / 阅读）。
//
// 抽成纯函数是为了让 `⌘E` 的**行为**可无头验证：它要答的两个问题——「按下之后落到哪个
// 模式」与「哪一个编辑器该响应」——都不依赖 DOM，而此前二者混在一个 `useEffect` 里，
// 只能靠真机点一遍。

import type { EditorViewModeDto } from "../../ipc/bindings";

/**
 * `⌘E` 的落点：编辑型（Live / 源码）→ 阅读；阅读 → **最近一次的编辑型模式**。
 *
 * 回到 `lastEditMode` 而不是恒回 Live：源码模式的用户切去读一眼再切回来，不该被
 * 顺手改成 Live（Obsidian 同心智）。
 */
export function toggledViewMode(
  current: EditorViewModeDto,
  lastEditMode: EditorViewModeDto,
): EditorViewModeDto {
  return current === "reading" ? lastEditMode : "reading";
}

/**
 * 本编辑器是否响应编辑器作用域的快捷键。
 *
 * 判据是**状态**（谁是活跃编辑器）而不是 DOM 焦点：阅读模式是只读 React 视图、没有
 * 可获焦的正文，用户点一下正文焦点就落在 `document.body` 上——按"事件冒泡到本编辑器
 * 子树"来判定，阅读模式下永远判不到，于是切进去就再也切不出来。多 pane 时后台 pane
 * 同样在监听 `window`，靠这个谓词让位给活跃 pane。
 *
 * 文件夹页签与空标签没有"正文"可切，一并不响应。
 */
export function respondsToEditorShortcuts(input: {
  active: boolean;
  path: string | null;
}): boolean {
  return input.active && input.path !== null && input.path.endsWith(".md");
}
