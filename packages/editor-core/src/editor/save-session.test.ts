import { ChangeSet, EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { changeOverEdits, editorDocumentText, minimalChange, rebaseCreatedDraft, serializedSave } from "./save-session";

describe("编辑器写出的正文（PRD-21-020 逐字节一致）", () => {
  it("原样写出：末尾换行、frontmatter、行首行尾空白一个字节都不补不删", () => {
    for (const doc of ["", "正文", "正文\n", "正文\n\n", "---\nluna:\n  type: note\n---\n  缩进 \t\n"]) {
      expect(editorDocumentText(EditorState.create({ doc }))).toBe(doc);
    }
  });

  it("行尾一律 `\\n`：装载进来的 `\\r\\n` / `\\r` 写出时都是 `\\n`", () => {
    expect(editorDocumentText(EditorState.create({ doc: "甲\r\n乙\r丙\n" }))).toBe("甲\n乙\n丙\n");
  });

  it("编辑之后照样原样（插入的 `\\r\\n` 同样按换行算）", () => {
    const state = EditorState.create({ doc: "甲" }).update({ changes: { from: 1, insert: "\r\n乙" } }).state;
    expect(editorDocumentText(state)).toBe("甲\n乙");
  });
});

describe("save session", () => {
  it("建档期间继续输入，映射到含元数据的保存结果", () => {
    const original = "草稿";
    const saved = "---\nluna:\n  type: quick-note\n---\n草稿";
    const edits = ChangeSet.of({ from: 2, insert: "继续输入" }, original.length);
    expect(rebaseCreatedDraft(original, saved, edits)).toBe(`${saved}继续输入`);
  });
  it("建档期间删除与插入均保留，普通笔记不注入元数据", () => {
    const edits = ChangeSet.of({ from: 1, to: 3, insert: "改变" }, 4);
    expect(rebaseCreatedDraft("abcd", "abcd", edits)).toBe("a改变d");
  });
  it("最小改动只换首尾之间不同的那一段", () => {
    const change = minimalChange("甲乙丙丁", "甲新丁");
    const spans: Array<[number, number, string]> = [];
    change.iterChanges((fromA, toA, _fromB, _toB, inserted) => spans.push([fromA, toA, inserted.toString()]));
    expect(spans).toEqual([[1, 3, "新"]]);
    expect(minimalChange("同", "同").empty).toBe(true);
  });
  it("别处的改动落到此刻的文档上：保存在途时接着打的字一个不丢", () => {
    const original = "第一段\n\n第二段";
    const merged = "第一段（别处）\n\n第二段";
    // 在途期间在末尾接着打了字。
    const edits = ChangeSet.of({ from: original.length, insert: "，接着写" }, original.length);
    const current = edits.apply(EditorState.create({ doc: original }).doc);
    expect(changeOverEdits(original, merged, edits).apply(current).toString()).toBe("第一段（别处）\n\n第二段，接着写");
  });
  it("重复 flush 共用保存任务，并等待期间新增内容", async () => {
    let release!: () => void;
    let dirty = true;
    let calls = 0;
    const save = serializedSave(async () => {
      calls++;
      if (calls === 1) await new Promise<void>((resolve) => { release = resolve; });
      else dirty = false;
      return true;
    }, () => dirty);
    const first = save();
    expect(save()).toBe(first);
    release();
    expect(await first).toBe(true);
    expect(calls).toBe(2);
  });
  it("失败或冲突停止重试，下一次显式操作可以重试", async () => {
    let calls = 0;
    const save = serializedSave(async () => { calls++; return false; }, () => true);
    expect(await save()).toBe(false);
    expect(calls).toBe(1);
    expect(await save()).toBe(false);
    expect(calls).toBe(2);
  });
});
