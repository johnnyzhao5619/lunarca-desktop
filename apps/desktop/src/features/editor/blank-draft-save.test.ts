import { describe, expect, it } from "vitest";

// 还没落盘的画布里只有空白时不去建笔记（E4 键盘可达真机 2026-10-06：Tab 进编辑区敲出的缩进让
// 状态栏挂着一句「错误」——Rust 拒绝建一篇只有空白的笔记）。建失败时报具体原因而非笼统「错误」。
// VaultEditor 没有渲染级测试夹具，这里按源码锚定保存流程里的两处。

const editors = import.meta.glob("./VaultEditor.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;
const EDITOR = Object.values(editors)[0];

describe("未落盘画布的保存", () => {
  it("扫描面非空", () => {
    expect(EDITOR.length).toBeGreaterThan(200);
  });

  it("只有空白时直接视为没有可存的东西，排在建笔记之前", () => {
    const guard = EDITOR.indexOf('if (!currentPath && content.trim() === "") {');
    const create = EDITOR.indexOf("commands.createNoteFromText(content");
    expect(guard).toBeGreaterThan(-1);
    expect(guard).toBeLessThan(create);
    expect(EDITOR.slice(guard, guard + 160)).toContain("markDirty(false)");
  });

  it("建失败时按错误说原因", () => {
    const create = EDITOR.indexOf("commands.createNoteFromText(content");
    const tail = EDITOR.slice(create, EDITOR.indexOf("const savedBeforeWrite", create));
    expect(tail).toContain("onStatus(vaultErrorText(createResult.error))");
    expect(tail).not.toContain('onStatus(t("error"))');
  });
});
