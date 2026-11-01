// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { blockMoveSpec } from "./block-move";
import { fullyParsed, parseFully } from "./syntax-tree-test-fixture";

function move(doc: string, cursorAt: string, up: boolean): string | "blocked" | null {
  const state = fullyParsed(EditorState.create({ doc, selection: { anchor: doc.indexOf(cursorAt) }, extensions: [markdown({ base: markdownLanguage })] }));
  const spec = blockMoveSpec(state, up);
  if (spec === null || spec === "blocked") return spec;
  return state.update({ changes: spec.changes }).state.doc.toString();
}

describe("⌥↑↓ 按块移动（对标方案 B2）", () => {
  it("列表项连同子项一起与同级项交换", () => {
    const doc = "- 甲\n  - 甲一\n  - 甲二\n- 乙\n- 丙";
    expect(move(doc, "乙", true)).toBe("- 乙\n- 甲\n  - 甲一\n  - 甲二\n- 丙");
    expect(move(doc, "甲", false)).toBe("- 乙\n- 甲\n  - 甲一\n  - 甲二\n- 丙");
    expect(move(doc, "甲二", true)).toBe("- 甲\n  - 甲二\n  - 甲一\n- 乙\n- 丙");
  });

  it("没有同级项可换时原地不动（不交给逐行移动拆散结构）", () => {
    expect(move("- 甲\n- 乙", "甲", true)).toBe("blocked");
    expect(move("- 甲\n  - 甲一\n- 乙", "甲一", false)).toBe("blocked");
  });

  it("表格、引用块、代码块围栏行：整块与相邻块交换；代码行与普通段落交还逐行移动", () => {
    const doc = "段落\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n> 引用\n> 两行";
    expect(move(doc, "| 1", true)).toBe("| a | b |\n| - | - |\n| 1 | 2 |\n\n段落\n\n> 引用\n> 两行");
    expect(move(doc, "两行", true)).toBe("段落\n\n> 引用\n> 两行\n\n| a | b |\n| - | - |\n| 1 | 2 |");
    const code = "段落\n\n```js\nlet a\nlet b\n```";
    expect(move(code, "```js", true)).toBe("```js\nlet a\nlet b\n```\n\n段落");
    expect(move(code, "let b", true)).toBeNull();
    expect(move(doc, "段落", false)).toBeNull();
  });
});

describe("有序列表移动后重编号", () => {
  it("同一笔事务里重编号，一步撤销", async () => {
    const { EditorView } = await import("@codemirror/view");
    const { history, undo } = await import("@codemirror/commands");
    const { moveBlock } = await import("./block-move");
    const doc = "1. 甲\n2. 乙\n3. 丙";
    const view = new EditorView({
      state: EditorState.create({ doc, selection: { anchor: doc.indexOf("丙") }, extensions: [markdown({ base: markdownLanguage }), history()] }),
    });
    parseFully(view);
    expect(moveBlock(view, true)).toBe(true);
    expect(view.state.doc.toString()).toBe("1. 甲\n2. 丙\n3. 乙");
    undo(view);
    expect(view.state.doc.toString()).toBe(doc);
    view.destroy();
  });
});
