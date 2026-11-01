import { EditorState, type ChangeSpec } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { describe, expect, it } from "vitest";
import { codeFenceEnterSpec, isInsideFencedCode } from "./code-fence-editing";

function stateAt(doc: string, anchor: number): EditorState {
  return EditorState.create({
    doc,
    selection: { anchor },
    extensions: [markdown({ base: markdownLanguage })],
  });
}

function applySpec(state: EditorState, spec: NonNullable<ReturnType<typeof codeFenceEnterSpec>>) {
  return state.update({
    changes: { from: spec.from, to: spec.to, insert: spec.insert } as ChangeSpec,
    selection: { anchor: spec.anchor },
  }).state;
}

describe("围栏代码 Enter 语义（PRD-06-011）", () => {
  it("在手输的开启围栏行末 Enter 自动补齐结束围栏并进入正文", () => {
    const state = stateAt("```sql", 6);
    const spec = codeFenceEnterSpec(state);
    expect(spec).not.toBeNull();
    const next = applySpec(state, spec!);
    expect(next.doc.toString()).toBe("```sql\n\n```");
    expect(next.selection.main.head).toBe(7);
  });

  it("在未闭合围栏的最后空行 Enter 自动封闭并退出到普通段落", () => {
    const doc = "```sql\nSELECT 1;\n";
    const state = stateAt(doc, doc.length);
    const spec = codeFenceEnterSpec(state);
    expect(spec).not.toBeNull();
    const next = applySpec(state, spec!);
    expect(next.doc.toString()).toBe("```sql\nSELECT 1;\n```\n");
    expect(next.selection.main.head).toBe(next.doc.length);
    expect(isInsideFencedCode(next, next.selection.main.head)).toBe(false);
  });

  it("在已闭合围栏前的最后空行 Enter 移出代码块且不复制结束围栏", () => {
    const doc = "```python\nprint('ok')\n\n```";
    const cursor = doc.indexOf("\n\n```") + 1;
    const state = stateAt(doc, cursor);
    const spec = codeFenceEnterSpec(state);
    expect(spec).not.toBeNull();
    const next = applySpec(state, spec!);
    expect(next.doc.toString()).toBe("```python\nprint('ok')\n```\n");
    expect(next.selection.main.head).toBe(next.doc.length);
  });

  it("代码正文的普通 Enter 交还给 CodeMirror，保留代码的换行与缩进语义", () => {
    const doc = "```python\n  return value\n```";
    const cursor = doc.indexOf("value") + "value".length;
    expect(codeFenceEnterSpec(stateAt(doc, cursor))).toBeNull();
  });

  it("识别波浪线围栏与缩进，退出时用同一种结束围栏", () => {
    const doc = "  ~~~js\n  console.log(1)\n";
    const state = stateAt(doc, doc.length);
    const spec = codeFenceEnterSpec(state);
    expect(spec).not.toBeNull();
    expect(applySpec(state, spec!).doc.toString()).toBe("  ~~~js\n  console.log(1)\n  ~~~\n");
  });
});
