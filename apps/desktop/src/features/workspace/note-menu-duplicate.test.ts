// 「创建副本」「用默认应用打开」（PRD-06-006，V5a）：只在调用方声明时出现（文件夹不给），多选不给；
// 命名与复制在 Rust，副本路径交回调用方。
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { noteMenuItems, type NoteMenuOptions } from "./note-menu";
import type { ContextMenuItem } from "../../lib/context-menu";

const t = (key: string) => key;

function menu(overrides: Partial<NoteMenuOptions> = {}): ContextMenuItem[] {
  return noteMenuItems(
    { path: "Notes/a.md", onStatus: () => {}, ...overrides },
    t,
    [],
    async () => false,
    async () => null,
    "",
  );
}

afterEach(() => {
  resetBindingsDouble();
});

describe("创建副本 / 用默认应用打开", () => {
  it("调用方不声明就没有；多选也没有", () => {
    expect(menu().map((item) => item.id)).not.toContain("duplicate");
    expect(menu().map((item) => item.id)).not.toContain("openExternally");
    const batch = menu({
      paths: ["Notes/a.md", "Notes/b.md"],
      duplicate: { onDuplicated: () => {} },
      openExternally: true,
    }).map((item) => item.id);
    expect(batch).not.toContain("duplicate");
    expect(batch).not.toContain("openExternally");
  });

  it("创建副本把 Rust 给的路径交回调用方；用默认应用打开走 open_vault_path", async () => {
    setCommandReply("duplicateVaultEntry", { status: "ok", data: "Notes/a 副本.md" });
    setCommandReply("openVaultPath", { status: "ok", data: null });
    const onDuplicated = vi.fn();
    const items = menu({ duplicate: { onDuplicated }, openExternally: true });
    const duplicate = items.find((item) => item.id === "duplicate");
    const external = items.find((item) => item.id === "openExternally");
    if (duplicate?.kind !== "action" || external?.kind !== "action") throw new Error("缺菜单项");
    duplicate.onSelect();
    external.onSelect();
    await vi.waitFor(() => expect(onDuplicated).toHaveBeenCalledWith("Notes/a 副本.md"));
    expect(commandMock("duplicateVaultEntry")).toHaveBeenCalledWith("Notes/a.md");
    expect(commandMock("openVaultPath")).toHaveBeenCalledWith("Notes/a.md");
  });
});
