// @vitest-environment happy-dom
/**
 * 离线锁定时封存的改动（TDD-17 §4）：解锁后放回来接着存，第一次上编辑器时换成带扩展的状态。
 */

import { EditorState } from "@codemirror/state";
import { describe, expect, it, vi } from "vitest";
import type { SaveResult, SealedDraft } from "../../worker/protocol";
import { SessionRegistry } from "./session-registry";

const draft: SealedDraft = { file_id: "f", path: "甲.md", base_rev: 3, saved_text: "旧", text: "新的改动" };

function registry(save: () => Promise<SaveResult>) {
  const call = vi.fn(async (cmd: string) => {
    if (cmd === "save") return save();
    throw new Error(cmd);
  });
  return {
    call,
    registry: new SessionRegistry({
      call: call as never,
      conflictLabel: () => "冲突 网页",
      setTimeout: (run, ms) => setTimeout(run, ms),
      clearTimeout: (handle) => clearTimeout(handle as number),
    }),
  };
}

const flushed = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("封存的改动放回来", () => {
  it("以封存时的基线接着存，存上了就放掉", async () => {
    const { registry: r, call } = registry(async () => ({ kind: "saved", rev: 4, path: "甲.md" }) as unknown as SaveResult);
    r.restore([draft]);
    await flushed();
    expect(call).toHaveBeenCalledWith("save", expect.objectContaining({ file_id: "f", base_rev: 3, text: "新的改动" }));
    await flushed();
    expect(r.get("f")).toBeUndefined();
    expect(r.hasUnsaved()).toBe(false);
  });

  it("还没存上时：仍算没保存，封存所需的基线都在", async () => {
    const { registry: r } = registry(() => new Promise<SaveResult>(() => {}));
    r.restore([draft]);
    await flushed();
    expect(r.hasUnsaved()).toBe(true);
    expect(r.unsavedDrafts()).toEqual([draft]);
  });

  it("第一次打开：换成编辑器带扩展的状态，正文仍是封存的那份，不进撤销历史", async () => {
    const { registry: r } = registry(() => new Promise<SaveResult>(() => {}));
    r.restore([draft]);
    await flushed();
    const session = r.get("f");
    expect(session?.restored).toBe(true);
    const opened = r.open({ file_id: "f", path: "甲.md", rev: 9, text: "云端更新的" } as never, EditorState.create({ doc: "云端更新的" }));
    expect(opened).toBe(session);
    expect(opened.restored).toBe(false);
    expect(opened.text()).toBe("新的改动");
    expect(opened.baseRev).toBe(3);
  });
});
