import { describe, expect, it } from "vitest";

// 脚本工具栏「AI 改稿」（ADR-165 ⑦）的编辑器一侧。段的范围是纯函数（`script-section-range.ts`，
// 单测在旁边）；VaultEditor 没有渲染级测试夹具，这里按源码锚定接线：只认活跃编辑器与同一篇、
// 没选区时按 Rust 分段取光标所在那一段、走与选区气泡同一个 AI 面板并带上要跑的动作。

const editors = import.meta.glob("./VaultEditor.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;
const menus = import.meta.glob("./FloatingMenu.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 去掉注释再断言：注释里提到的名字不能替代真实接线。 */
function code(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
}

const EDITOR = code(Object.values(editors)[0]);
const start = EDITOR.lastIndexOf("useEffect(", EDITOR.indexOf("window.addEventListener(SCRIPT_REWRITE_EVENT, onRewrite)"));
const REWRITE = EDITOR.slice(start, EDITOR.indexOf("});", EDITOR.indexOf("removeEventListener(SCRIPT_REWRITE_EVENT", start)));
const MENU = code(Object.values(menus)[0]);

describe("脚本工具栏 AI 改稿：编辑器接线", () => {
  it("扫描面非空", () => {
    expect(REWRITE.length).toBeGreaterThan(200);
  });

  it("只认活跃编辑器、同一篇、可编辑的缓冲区", () => {
    expect(REWRITE).toMatch(/if \(!respondsToEditorShortcuts\(\{ active, path \}\)\) \{\s*return;/);
    expect(REWRITE).toContain("detail.path !== pathRef.current");
    expect(REWRITE).toContain("view.state.readOnly");
  });

  it("没有选区时按 Rust 的分段取光标所在那一段，再以选区作用域打开面板并带上动作", () => {
    expect(REWRITE).toContain("scriptSectionRangeAt(view.state.doc.toString(), starts, main.head)");
    expect(REWRITE).toContain("liveScriptRef.current?.sections");
    expect(REWRITE).toContain("openSelectionSession(");
    expect(REWRITE).toContain("autoRun: detail.action");
  });

  it("面板拿到动作就跑一次，与手点同一条 runAction", () => {
    expect(MENU).toMatch(/if \(autoRun && !autoRanRef\.current\) \{\s*autoRanRef\.current = true;\s*runAction\(autoRun\);/);
  });
});
