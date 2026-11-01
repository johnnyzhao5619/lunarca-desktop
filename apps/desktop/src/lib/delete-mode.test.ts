import { describe, expect, it } from "vitest";
import { deleteConsequenceKey, trashViewState } from "./delete-mode";

describe("deleteConsequenceKey", () => {
  it("三档策略各说各的落点", () => {
    expect(deleteConsequenceKey("trash")).toBe("main.deleteConsequence.trash");
    expect(deleteConsequenceKey("vault_trash")).toBe("main.deleteConsequence.vaultTrash");
    expect(deleteConsequenceKey("permanent")).toBe("main.deleteConsequence.permanent");
  });

  // 未解析时**不许回落到任何一档**：猜「可从回收站还原」正是这个模块要根治的 bug。
  it("策略未解析时不承诺可恢复性", () => {
    expect(deleteConsequenceKey(null)).toBe("main.deleteConsequence.unknown");
  });
});

describe("trashViewState", () => {
  it("库内回收站生效即正常清单（有没有条目都是清单，空态由列表自己答）", () => {
    expect(trashViewState("vault_trash", 0)).toBe("list");
    expect(trashViewState("vault_trash", 3)).toBe("list");
  });

  it("系统废纸篓 / 永久删除且库内无条目时各给各的说明卡", () => {
    expect(trashViewState("trash", 0)).toBe("system");
    expect(trashViewState("permanent", 0)).toBe("permanent");
  });

  // 改回系统废纸篓之后，此前留在 `.trash/` 里的条目**不许被藏起来**——它们还占着磁盘，
  // 而且还能还原。
  it("策略已切走但库内仍有遗留条目时照列不误", () => {
    expect(trashViewState("trash", 2)).toBe("listLegacy");
    expect(trashViewState("permanent", 2)).toBe("listLegacy");
  });

  it("策略未解析时不预判形态", () => {
    expect(trashViewState(null, 0)).toBe("pending");
    expect(trashViewState(null, 5)).toBe("pending");
  });
});
