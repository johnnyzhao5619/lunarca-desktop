import { describe, expect, it } from "vitest";

// 退出与安装更新前的保存回执（TDD-13 §5），编辑面这一侧怎么回答。VaultEditor 没有渲染级测试夹具，
// 这里按源码锚定：保存屏障走的是显式保存，不看自动保存开关；写盘失败与外部冲突都必须回答 false，
// 否则回执协议会把没落盘的内容当成已保存，然后重启。协议本身的行为见 use-editor-exit-guard.test.tsx。

const editors = import.meta.glob("./VaultEditor.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 去掉注释再断言：注释里提到的名字不能替代真实接线。 */
function code(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
}

const EDITOR = code(Object.values(editors)[0]);
const SAVE_ACTION = EDITOR.slice(
  EDITOR.indexOf("saveActionRef.current = async () => {"),
  EDITOR.indexOf("}, [onSavedPath, onStatus, t]);"),
);
const WRITE = SAVE_ACTION.slice(SAVE_ACTION.indexOf("commands.writeNote(currentPath"));

describe("编辑面在保存屏障上的回答", () => {
  it("扫描面非空", () => {
    expect(SAVE_ACTION.length).toBeGreaterThan(400);
    expect(WRITE.length).toBeGreaterThan(200);
  });

  it("屏障调用的是串行化的显式保存", () => {
    expect(EDITOR).toContain("registerEditorSave(() => flushRef.current(),");
    expect(EDITOR).toContain("serializedSave(() => saveActionRef.current(), () => dirtyRef.current)");
  });

  it("自动保存关闭时屏障照样写盘：保存动作不看自动保存开关", () => {
    expect(SAVE_ACTION).not.toMatch(/autosave/i);
    expect(EDITOR).toMatch(/if \(!autosaveEnabled \|\| !dirty/);
  });

  it("外部改动待处理时不写、回答失败", () => {
    expect(SAVE_ACTION).toContain("if (externalPendingRef.current) return false;");
  });

  it("写入发现外部冲突时回答失败，并把横幅打开", () => {
    const conflict = WRITE.slice(WRITE.indexOf('result.data.kind === "conflict"'));
    const branch = conflict.slice(0, conflict.indexOf("return false;"));
    expect(branch).toContain("externalPendingRef.current = true");
    expect(branch).toContain("setExternalPending(true)");
    expect(branch).not.toContain("return true");
  });

  it("写盘失败回答失败，且不把基线推进到没写成的内容", () => {
    const failed = WRITE.slice(WRITE.lastIndexOf("savedContentRef.current = savedBeforeWrite;"));
    expect(failed).toContain("markDirty(currentContent !== savedBeforeWrite)");
    expect(failed).toContain("return false;");
    expect(failed).not.toContain("return true");
  });
});
