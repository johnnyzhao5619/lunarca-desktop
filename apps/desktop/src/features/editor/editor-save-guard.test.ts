import { describe, expect, it } from "vitest";
import { flushEditorSaves, holdEditorInputs, registerEditorSave, withEditorSaveGuard } from "./editor-save-guard";
describe("editor navigation save barrier", () => {
  it("退出请求期间锁定编辑输入与导航，取消后恢复", async () => {
    let held = false; let moved = false;
    const dispose = registerEditorSave(async () => true, (value) => { held = value; });
    holdEditorInputs(true); expect(held).toBe(true);
    expect(await withEditorSaveGuard(() => { moved = true; })).toBe(false); expect(moved).toBe(false);
    holdEditorInputs(false); expect(held).toBe(false);
    expect(await withEditorSaveGuard(() => { moved = true; })).toBe(true); dispose();
  });
  it("任一分屏冲突时阻止移除；成功必须等保存回执", async () => {
    let resolve!: (saved: boolean) => void;
    let held = false;
    const dispose = registerEditorSave(() => new Promise<boolean>((done) => { resolve = done; }), (value) => { held = value; });
    let closed = false;
    const pending = withEditorSaveGuard(() => { closed = true; });
    expect(closed).toBe(false); expect(held).toBe(true); resolve(false);
    expect(await pending).toBe(false); expect(closed).toBe(false); expect(held).toBe(false);
    const success = withEditorSaveGuard(() => { closed = true; }); resolve(true);
    expect(await success).toBe(true); expect(closed).toBe(true); dispose();
  });
  it("失效保存不允许关闭，卸载后不留旧处理器", async () => {
    const dispose = registerEditorSave(async () => { throw new Error("write failed"); });
    expect(await flushEditorSaves()).toBe(false); dispose();
    expect(await flushEditorSaves()).toBe(true);
  });
  it("保存期间到达退出请求时，取消导航且不能提前解锁输入", async () => {
    let resolve!: (saved: boolean) => void; let held = false; let moved = false;
    const dispose = registerEditorSave(() => new Promise<boolean>((done) => { resolve = done; }), (value) => { held = value; });
    const pending = withEditorSaveGuard(() => { moved = true; });
    holdEditorInputs(true); resolve(true); expect(await pending).toBe(false);
    expect(moved).toBe(false); expect(held).toBe(true);
    holdEditorInputs(false); expect(held).toBe(false); dispose();
  });
});
