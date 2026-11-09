// @vitest-environment happy-dom
/**
 * 一扇窗对 Rust 保存回执协议的应答（退出与安装更新共用，TDD-13 §5）：
 * 1. 收到请求先冻结输入，保存回执带上结果；
 * 2. 保存成功后输入继续冻结（等进程换掉），只有 Rust 发 cancelled 才解冻；
 * 3. 保存失败、抛错都回执 false，输入等 cancelled 解冻，编辑器里的内容不动。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, renderHook, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, emitEvent, listenerCount, resetBindingsDouble } from "../../ipc/bindings-test-double";
import { registerEditorSave } from "./editor-save-guard";
import { useEditorExitGuard } from "./use-editor-exit-guard";

const disposers: (() => void)[] = [];

function editor(save: () => Promise<boolean>) {
  const state = { held: false };
  disposers.push(registerEditorSave(save, (held) => { state.held = held; }));
  return state;
}

async function mountGuard() {
  renderHook(() => useEditorExitGuard());
  await waitFor(() => {
    expect(listenerCount("editorFlushRequested")).toBe(1);
    expect(listenerCount("editorFlushCancelled")).toBe(1);
  });
}

beforeEach(resetBindingsDouble);
afterEach(() => {
  cleanup();
  while (disposers.length > 0) disposers.pop()?.();
});

describe("编辑窗口的保存回执", () => {
  it("同窗多个编辑面都保存成功才回执 true，之后输入继续冻结", async () => {
    const left = editor(async () => true);
    const right = editor(async () => true);
    await mountGuard();
    emitEvent("editorFlushRequested", { request_id: 7 });
    await waitFor(() => expect(commandMock("editorFlushReply")).toHaveBeenCalledWith(7, true));
    expect(left.held).toBe(true);
    expect(right.held).toBe(true);
  });

  it("回执要等保存真的结束，保存期间已经冻结", async () => {
    let finish!: (saved: boolean) => void;
    const surface = editor(() => new Promise<boolean>((done) => { finish = done; }));
    await mountGuard();
    emitEvent("editorFlushRequested", { request_id: 8 });
    await waitFor(() => expect(surface.held).toBe(true));
    expect(commandMock("editorFlushReply")).not.toHaveBeenCalled();
    finish(true);
    await waitFor(() => expect(commandMock("editorFlushReply")).toHaveBeenCalledWith(8, true));
  });

  it("任一编辑面保存失败（写盘失败或外部冲突）回执 false，cancelled 后解冻", async () => {
    const saved = editor(async () => true);
    const conflicted = editor(async () => false);
    await mountGuard();
    emitEvent("editorFlushRequested", { request_id: 9 });
    await waitFor(() => expect(commandMock("editorFlushReply")).toHaveBeenCalledWith(9, false));
    expect(conflicted.held).toBe(true);
    emitEvent("editorFlushCancelled", null);
    expect(conflicted.held).toBe(false);
    expect(saved.held).toBe(false);
  });

  it("保存抛错同样回执 false，不会卡住", async () => {
    editor(async () => { throw new Error("disk full"); });
    await mountGuard();
    emitEvent("editorFlushRequested", { request_id: 10 });
    await waitFor(() => expect(commandMock("editorFlushReply")).toHaveBeenCalledWith(10, false));
  });
});
