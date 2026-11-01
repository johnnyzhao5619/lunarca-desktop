/**
 * 多选代数（PRD-06-059）。它是纯函数，故这里钉的是**规则本身**，不是某个清单的渲染。
 *
 * 每条用例对应一次会被用户当场察觉的错：连选并上了上一次的残留、右键把刚选好的十条
 * 清空、折叠一个文件夹之后"删除 5 项"删掉了看不见的对象。
 */

import { describe, expect, it } from "vitest";
import {
  EMPTY_SELECTION,
  orderedSelection,
  prunedSelection,
  rowAtOffset,
  selectionAfterClick,
  selectionForContextMenu,
  singleSelection,
} from "./note-selection";

const ROWS = ["a.md", "b.md", "c.md", "d.md"];
const plain = { toggle: false, range: false };
const toggle = { toggle: true, range: false };
const range = { toggle: false, range: true };

function paths(state: { paths: ReadonlySet<string> }): string[] {
  return [...state.paths];
}

describe("多选代数", () => {
  it("无修饰键 = 单选，并把锚点落在这一行", () => {
    const state = selectionAfterClick(singleSelection("a.md"), "c.md", plain, ROWS);
    expect(paths(state)).toEqual(["c.md"]);
    expect(state.anchor).toBe("c.md");
  });

  it("Cmd/Ctrl = 加选与去选，锚点跟到最后点的那一行", () => {
    const added = selectionAfterClick(singleSelection("a.md"), "c.md", toggle, ROWS);
    expect(paths(added).sort()).toEqual(["a.md", "c.md"]);
    expect(added.anchor).toBe("c.md");
    // 再点一次同一行 = 去选（不是把它选两遍）。
    expect(paths(selectionAfterClick(added, "c.md", toggle, ROWS))).toEqual(["a.md"]);
  });

  it("连选的两端都在状态里：锚点不动，动端跟着最后一次走", () => {
    // 第一版把 DOM 焦点当动端，于是焦点没跟上时，连按两次 Shift+↓ 每次都从锚点重新取
    // 同一段——选区停在原地（2026-09-02 真机：Shift+方向键"无反应"的另一半）。
    let state = singleSelection("b.md");
    state = selectionAfterClick(state, "c.md", range, ROWS);
    expect(state.anchor).toBe("b.md");
    expect(state.lead).toBe("c.md");
    // 下一次连选从动端继续（方向键据它算下一行），而起点仍是那个锚点。
    state = selectionAfterClick(state, "d.md", range, ROWS);
    expect(paths(state)).toEqual(["b.md", "c.md", "d.md"]);
    expect(state.anchor).toBe("b.md");
    expect(state.lead).toBe("d.md");
    // 收敛也要带上两端：不在场的动端会让下一次方向键从头开始。
    const pruned = prunedSelection(state, ["a.md", "b.md"]);
    expect(pruned.lead).toBeNull();
    expect(pruned.anchor).toBe("b.md");
  });

  it("Shift = 从锚点连选到落点，且**替换**而不是并上上一次的残留", () => {
    const start = selectionAfterClick(EMPTY_SELECTION, "b.md", plain, ROWS);
    const extended = selectionAfterClick(start, "d.md", range, ROWS);
    expect(paths(extended)).toEqual(["b.md", "c.md", "d.md"]);
    // 锚点不动：连着按 Shift 点几次，每次都从同一个起点重新取区间。
    expect(extended.anchor).toBe("b.md");
    const shrunk = selectionAfterClick(extended, "c.md", range, ROWS);
    expect(paths(shrunk)).toEqual(["b.md", "c.md"]);
    // 反方向同样成立（区间是无序的两端）。
    expect(paths(selectionAfterClick(extended, "a.md", range, ROWS))).toEqual(["a.md", "b.md"]);
  });

  it("没有锚点时的 Shift 退化成单选（而不是选中半份列表）", () => {
    expect(paths(selectionAfterClick(EMPTY_SELECTION, "c.md", range, ROWS))).toEqual(["c.md"]);
  });

  it("右键落在选中集里保持整批，落在集外才单选它", () => {
    const many = selectionAfterClick(singleSelection("a.md"), "b.md", toggle, ROWS);
    expect(paths(selectionForContextMenu(many, "b.md")).sort()).toEqual(["a.md", "b.md"]);
    expect(paths(selectionForContextMenu(many, "d.md"))).toEqual(["d.md"]);
  });

  it("看不见的行不算选中：可见行一变就收敛，锚点跟着收敛", () => {
    const many = selectionAfterClick(singleSelection("a.md"), "c.md", toggle, ROWS);
    const after = prunedSelection(many, ["a.md", "b.md"]);
    expect(paths(after)).toEqual(["a.md"]);
    expect(after.anchor).toBeNull();
    // 没有变化时返回同一个对象（不制造无谓的重渲染）。
    expect(prunedSelection(after, ["a.md", "b.md"])).toBe(after);
  });

  it("提交顺序恒按所见顺序，不按勾选先后（PRD-06-047 的既有裁决）", () => {
    let state = singleSelection("d.md");
    state = selectionAfterClick(state, "a.md", toggle, ROWS);
    state = selectionAfterClick(state, "c.md", toggle, ROWS);
    expect(orderedSelection(state, ROWS)).toEqual(["a.md", "c.md", "d.md"]);
  });

  it("方向键在两端停住，不循环（列表两头是真实的边界）", () => {
    expect(rowAtOffset(ROWS, "b.md", 1)).toBe("c.md");
    expect(rowAtOffset(ROWS, "a.md", -1)).toBe("a.md");
    expect(rowAtOffset(ROWS, "d.md", 1)).toBe("d.md");
    // 焦点还不在任何一行上时，↓ 从头开始、↑ 从尾开始。
    expect(rowAtOffset(ROWS, null, 1)).toBe("a.md");
    expect(rowAtOffset(ROWS, null, -1)).toBe("d.md");
    // 那一行已经不在场（刚被删/被折叠）：退化到同一条规则，不抛错。
    expect(rowAtOffset(ROWS, "已删除.md", 1)).toBe("a.md");
    expect(rowAtOffset([], "a.md", 1)).toBeNull();
  });
});
