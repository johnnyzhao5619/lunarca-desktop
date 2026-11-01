import { describe, expect, it } from "vitest";
import { graphScopeMenuState } from "./graph-scope-menu";

/**
 * 「AI 关系标签 ▸」文件夹子菜单的三态判定（PRD-06-035c）。
 *
 * 覆盖判断是 Rust `scope_covers` 的呈现镜像——边界必须同口径：`Chats` 覆盖
 * `Chats/sub`，绝不覆盖 `Chats归档`。三态互斥：本层被精确排除呈勾选、被祖先规则
 * 覆盖呈「已随 X 停用」禁用说明（在子文件夹上给一个点了不生效的开关是误导）、
 * 其余可用。
 */
describe("graphScopeMenuState（PRD-06-035c）", () => {
  it("无规则命中时为 eligible", () => {
    expect(graphScopeMenuState("Projects", [])).toEqual({ kind: "eligible" });
    expect(graphScopeMenuState("Projects", ["Chats"])).toEqual({ kind: "eligible" });
  });

  it("本层被精确排除时为 excluded", () => {
    expect(graphScopeMenuState("Chats", ["Chats"])).toEqual({ kind: "excluded" });
  });

  it("被祖先规则覆盖时指出规则挂在哪一层", () => {
    expect(graphScopeMenuState("Chats/2026", ["Chats"])).toEqual({
      kind: "coveredByAncestor",
      ancestor: "Chats",
    });
    // 深层子孙同样归因到祖先。
    expect(graphScopeMenuState("Chats/2026/一月", ["Chats"])).toEqual({
      kind: "coveredByAncestor",
      ancestor: "Chats",
    });
  });

  it("覆盖判断按路径段边界，不做裸前缀匹配", () => {
    // 这条一旦松掉，「排除 Chats」会让 `Chats归档` 的菜单也变成"已停用"。
    expect(graphScopeMenuState("Chats归档", ["Chats"])).toEqual({ kind: "eligible" });
    expect(graphScopeMenuState("Chats2", ["Chats"])).toEqual({ kind: "eligible" });
  });

  it("精确排除优先于祖先归因（自身在清单里就呈勾选态）", () => {
    expect(graphScopeMenuState("Chats/2026", ["Chats", "Chats/2026"])).toEqual({
      kind: "coveredByAncestor",
      ancestor: "Chats",
    });
    // 只有自身一条规则时是 excluded——解除勾选即可回到可用。
    expect(graphScopeMenuState("Chats/2026", ["Chats/2026"])).toEqual({ kind: "excluded" });
  });
});
