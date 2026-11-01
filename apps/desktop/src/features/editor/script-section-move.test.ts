import { describe, expect, it } from "vitest";

// 大纲拖动调段序（PRD-19-011）的编辑器一侧。搬段本身是 Rust 纯函数（luna-vault `move_section`，
// 单测在那里）；VaultEditor 没有渲染级测试夹具，这里按源码锚定四处接线：只认活跃编辑器、
// 等待期间状态变了就不落、结果作为一次可撤销的编辑落下、不抢大纲的焦点。

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
const effectStart = EDITOR.lastIndexOf("useEffect(", EDITOR.indexOf("SCRIPT_SECTION_MOVE_EVENT, onMove"));
const MOVE = EDITOR.slice(effectStart, EDITOR.indexOf("}, [active, path]);", effectStart));

describe("大纲拖动调段序：编辑器接线", () => {
  it("扫描面非空", () => {
    expect(MOVE.length).toBeGreaterThan(200);
    expect(MOVE).toContain("window.addEventListener(SCRIPT_SECTION_MOVE_EVENT, onMove)");
  });

  it("只认活跃编辑器、同一篇、可编辑的缓冲区", () => {
    expect(MOVE).toMatch(/if \(!respondsToEditorShortcuts\(\{ active, path \}\)\) \{\s*return;/);
    expect(MOVE).toContain("detail.path !== pathRef.current");
    expect(MOVE).toContain("view.state.readOnly");
  });

  it("搬段交给 Rust，等结果期间换了笔记或又打了字就不落", () => {
    expect(MOVE).toContain("commands.scriptMoveSection(before, detail.fromLine, detail.beforeLine)");
    const guard = MOVE.match(/if \(!result[^\n]*\) \{\s*return;/)?.[0] ?? "";
    expect(guard).toContain("pathRef.current !== detail.path");
    expect(guard).toContain("current.state.doc.toString() !== before");
  });

  it("结果作为一次可撤销的编辑落下，不绕过撤销栈，也不抢焦点", () => {
    expect(MOVE).toContain('current.dispatch({ changes: result.change, userEvent: "move.section" })');
    expect(MOVE).not.toContain("addToHistory");
    expect(MOVE).not.toContain("applyIncrementalText");
    expect(MOVE).not.toContain(".focus()");
    expect(MOVE).not.toContain("applyReveal");
  });
});
