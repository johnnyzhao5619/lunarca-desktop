import { describe, expect, it } from "vitest";
import { EditorState, Text } from "@codemirror/state";
import {
  aiRangeOf,
  aiSessionExtension,
  contextAround,
  CONTEXT_SLICE_MAX_CHARS,
  CURSOR_SLICE_MAX_CHARS,
  setAiRange,
  textBeforeCursor,
} from "./ai-session";

function stateWith(doc: string) {
  return EditorState.create({ doc, extensions: [aiSessionExtension()] });
}

describe("ai session frozen range", () => {
  it("is null until a session sets it", () => {
    expect(aiRangeOf(stateWith("hello"))).toBeNull();
  });

  it("follows insertions made before the range while a result is pending", () => {
    let state = stateWith("intro\n\nselected text\n\noutro");
    const from = "intro\n\n".length;
    const to = from + "selected text".length;
    state = state.update({ effects: setAiRange.of({ from, to }) }).state;

    // 等待期间在选区上方插入一段：范围整体后移，替换仍命中原来那段。
    state = state.update({ changes: { from: 0, insert: "NEW PARAGRAPH\n\n" } }).state;
    const mapped = aiRangeOf(state);
    expect(mapped).not.toBeNull();
    expect(state.sliceDoc(mapped!.from, mapped!.to)).toBe("selected text");
  });

  it("does not swallow text typed at the range edges", () => {
    let state = stateWith("abc");
    state = state.update({ effects: setAiRange.of({ from: 0, to: 3 }) }).state;
    state = state.update({ changes: { from: 3, insert: "d" } }).state;
    state = state.update({ changes: { from: 0, insert: "z" } }).state;
    const mapped = aiRangeOf(state)!;
    expect(state.sliceDoc(mapped.from, mapped.to)).toBe("abc");
  });

  it("clears on a null effect", () => {
    let state = stateWith("abc");
    state = state.update({ effects: setAiRange.of({ from: 0, to: 3 }) }).state;
    state = state.update({ effects: setAiRange.of(null) }).state;
    expect(aiRangeOf(state)).toBeNull();
  });
});

describe("contextAround", () => {
  it("returns the surrounding paragraph text without the selection", () => {
    const doc = Text.of(["first line", "second line", "", "far away"]);
    const from = "first line\n".length;
    const to = from + "second".length;
    expect(contextAround(doc, from, to)).toEqual({ before: "first line", after: "line" });
  });

  it("stops at blank lines in both directions", () => {
    const doc = Text.of(["p1", "", "a", "b", "c", "", "p3"]);
    const bFrom = "p1\n\na\n".length;
    expect(contextAround(doc, bFrom, bFrom + 1)).toEqual({ before: "a", after: "c" });
  });

  it("caps each side to the slice limit", () => {
    const long = "x".repeat(CONTEXT_SLICE_MAX_CHARS + 200);
    const doc = Text.of([long, "mid", long]);
    const from = long.length + 1;
    const ctx = contextAround(doc, from, from + 3);
    expect(ctx.before.length).toBe(CONTEXT_SLICE_MAX_CHARS);
    expect(ctx.after.length).toBe(CONTEXT_SLICE_MAX_CHARS);
  });
});

describe("textBeforeCursor", () => {
  it("returns the tail before the cursor, capped", () => {
    const doc = Text.of(["y".repeat(CURSOR_SLICE_MAX_CHARS + 10) + "END"]);
    const tail = textBeforeCursor(doc, doc.length);
    expect(tail.length).toBe(CURSOR_SLICE_MAX_CHARS);
    expect(tail.endsWith("END")).toBe(true);
  });
});
