import { ChangeSet } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { rebaseCreatedDraft, serializedSave } from "./save-session";

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
