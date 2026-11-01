/// <reference types="vite/client" />
/**
 * 右键菜单的调用方不得自己先 `preventDefault()` 再 `contextMenu.open(event, …)`。
 *
 * 宿主 `open` 见到已被拦下的事件就让位——那是「最内层表面先认领、外层让位」的唯一判据
 * （`ContextMenuHost`）。调用方自己先拦，等于宣布「已经有人认领」，于是自己的菜单也不弹。
 * 2026-10-05 真机：标签页右键（固定 / 关闭其他 / 关闭右侧）与书签行右键因此从没弹出来过，
 * 单测全绿——它们直接调菜单构建函数，没经过宿主。
 *
 * 判法按「处理函数体」：同一个 `onContextMenu={…}` 箭头函数或 `function x(event…)` 里，
 * `preventDefault()` 出现在 `contextMenu.open(event` 之前即违规。把事件换成新的
 * `MouseEvent` 再交给宿主的（编辑器拼写建议那条异步路径）不算。
 */
import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob(["../**/*.tsx", "!../**/*.test.tsx"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 从每个处理函数起点往后取一段，看 `preventDefault()` 是否先于把**同一个事件**交给宿主。 */
function selfPreventedOpens(sources: Record<string, string>): string[] {
  const found: string[] = [];
  const starts = /onContextMenu=\{\s*\(\s*(\w+)[^)]*\)\s*=>\s*\{|function\s+\w+\(\s*(\w+)\s*:[^)]*\)\s*\{/g;
  for (const [path, source] of Object.entries(sources)) {
    for (const match of source.matchAll(starts)) {
      const name = match[1] ?? match[2];
      const body = source.slice(match.index + match[0].length, match.index + match[0].length + 1200);
      const end = body.search(/\n\s{0,4}\}\n|\n\s*\}\}\n/);
      const handler = end === -1 ? body : body.slice(0, end);
      const prevent = handler.indexOf(`${name}.preventDefault()`);
      const open = handler.search(new RegExp(`\\.open\\(\\s*${name}\\b`));
      if (prevent !== -1 && open !== -1 && prevent < open) {
        found.push(`${path}:${source.slice(0, match.index).split("\n").length}`);
      }
    }
  }
  return found.sort();
}

describe("右键菜单调用方不自己拦事件", () => {
  it("全部源码：没有先 preventDefault 再交给宿主的处理函数", () => {
    expect(selfPreventedOpens(SOURCES)).toEqual([]);
  });

  it("反向夹具：先拦再开会被抓出来；只开不拦、换新事件再开都放行", () => {
    const fixture = {
      "bad-inline.tsx": "onContextMenu={(event: ReactMouseEvent) => {\n  event.preventDefault();\n  contextMenu.open(event, items);\n}}\n",
      "bad-function.tsx": "function openMenu(event: ReactMouseEvent, x: X) {\n  event.preventDefault();\n  contextMenu.open(\n    event,\n    items,\n  );\n}\n",
      "good.tsx": "onContextMenu={(event: ReactMouseEvent) => {\n  event.stopPropagation();\n  contextMenu.open(event, items);\n}}\n",
      "good-async.tsx":
        "function openEditorContextMenu(event: React.MouseEvent) {\n  event.preventDefault();\n  later(() => show(new MouseEvent(\"contextmenu\")));\n}\n",
    };
    expect(selfPreventedOpens(fixture)).toEqual(["bad-function.tsx:1", "bad-inline.tsx:1"]);
  });
});
