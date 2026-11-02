// @vitest-environment happy-dom
/**
 * 后端发起的导航在工作区挂上之前就到了，也要落地，且只落地一次（2026-10-15 Windows 真机）。
 *
 * 应用没在跑时点连接链接：Rust 在 setup 里就把「去账户页」发了出去，那时主窗的工作区还没挂上
 * 监听器，事件落地即丢——应用起来了，停在笔记。现在请求存在 Rust（`take_pending_navigation`
 * 取即清），工作区挂上监听后取一次、每收到一次事件再取一次。这里用真实的 `WorkspaceApp` 加一个
 * 模拟 Rust 单槽的替身，把三种先后次序都钉住：挂载前就存好、挂载后才到、事件抢在挂载那次取之前。
 *
 * 「落地」的判据是设置页真的收到了那个深链（`SettingsView` 的 `focus`），不只是派发了事件。
 */
import { createElement, StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onCloseRequested: () => Promise.resolve(() => {}),
    startDragging: () => Promise.resolve(),
  }),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));

/** 设置页每次渲染收到的深链焦点（`nonce` 每落地一次导航自增一次）。 */
const probe = vi.hoisted(() => ({ focus: [] as Array<{ route: string; nonce: number } | null> }));
vi.mock("../features/workspace/WorkspacePrimaryRail", () => ({
  WorkspacePrimaryRail: () => null,
}));
vi.mock("../features/editor/VaultEditor", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../features/editor/VaultEditor")>()),
  VaultEditor: () => null,
}));
vi.mock("../features/settings/SettingsView", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../features/settings/SettingsView")>()),
  SettingsView: (props: { focus: { route: string; nonce: number } | null }) => {
    probe.focus.push(props.focus);
    return null;
  },
}));

import { emitEvent, resetBindingsDouble, setCommandReply } from "../ipc/bindings-test-double";
import { withI18n } from "../i18n/i18n-test-fixture";
import { ConfirmHost } from "../components/ConfirmHost";
import { ContextMenuHost } from "../components/ContextMenuHost";
import { PromptHost } from "../components/PromptHost";
import { WorkspaceApp } from "./WorkspaceApp";

const ACCOUNT = "settings://account";

/** Rust 那一格 `PendingNavigation`：存覆盖、取即清。 */
let pendingInRust: string | null = null;

afterEach(() => {
  cleanup();
});
beforeEach(() => {
  resetBindingsDouble();
  // 一方应用清单（ADR-165）：普通值命令，替身不会自己给出数组。
  setCommandReply("appsList", []);
  probe.focus = [];
  pendingInRust = null;
  setCommandReply("takePendingNavigation", () => {
    const route = pendingInRust;
    pendingInRust = null;
    return route;
  });
  setCommandReply("processingQueueSnapshot", []);
  setCommandReply("listSettings", { status: "ok", data: [] });
  // 落到设置空间后设置控制器会取快捷键诊断；页面本身已换成探针。
  setCommandReply("shortcutDiagnostics", { status: "ok", data: [] });
  setCommandReply("treeViewState", { status: "ok", data: { sort: "name", expanded: [] } });
  setCommandReply("conflictCopies", { status: "ok", data: [] });
  const unavailable = { status: "error", error: "unavailable" };
  for (const name of ["chatAttach", "listDraftNotes", "chatHistoryList"]) {
    setCommandReply(name, unavailable);
  }
  for (const name of ["search", "bookmarks"]) {
    setCommandReply(name, { status: "ok", data: [] });
  }
  setCommandReply("listVaults", { status: "ok", data: { account: null, cloud: false, vaults: [] } });
  for (const name of ["vaultLinkPending", "externalRenamePrompt"]) {
    setCommandReply(name, null);
  }
  setCommandReply("recordingState", {
    status: "idle",
    purpose: "record",
    session_id: null,
    note_rel: null,
    elapsed_ms: 0,
    message_key: null,
    message_detail: null,
  });
  setCommandReply("noteTypeArchiveDir", { status: "ok", data: "Recordings" });
  setCommandReply("vaultListDirs", { status: "ok", data: [] });
});

function workspace(key: number) {
  return withI18n(
    createElement(
      ContextMenuHost,
      null,
      createElement(
        ConfirmHost,
        null,
        createElement(
          PromptHost,
          null,
          createElement(WorkspaceApp, {
            key,
            initialVault: { open: true, root: "/v", missing_root: null, missing_in_cloud: false },
            launchOverview: false,
          }),
        ),
      ),
    ),
  );
}

async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 50));
  });
}

/** 设置页落地过几次去账户页的导航（不同 nonce 的个数）。 */
function accountLandings(): number {
  const nonces = new Set(
    probe.focus.filter((focus) => focus?.route === ACCOUNT).map((focus) => focus?.nonce),
  );
  return nonces.size;
}

describe("后端发起的导航：先存后取，恰好落地一次", () => {
  it("冷启动：工作区挂上之前就存好了（事件早已丢失）——挂载即去账户页，重挂载不再去", async () => {
    pendingInRust = ACCOUNT;
    const view = render(workspace(0));
    await settle();
    expect(accountLandings()).toBe(1);
    expect(pendingInRust).toBeNull();

    // 切库会整体重挂载工作区：那一次已经落地，不能再落一次。
    probe.focus = [];
    view.rerender(workspace(1));
    await settle();
    expect(accountLandings()).toBe(0);
  });

  it("StrictMode 的挂载-卸载-再挂载：第一次挂载取走的那一次照样落地", async () => {
    pendingInRust = ACCOUNT;
    render(createElement(StrictMode, null, workspace(0)));
    await settle();
    expect(accountLandings()).toBe(1);
  });

  it("运行中：挂载时取空，之后存进来并发事件——随事件取到、落地一次，重复的事件不再落地", async () => {
    render(workspace(0));
    await settle();
    expect(accountLandings()).toBe(0);

    pendingInRust = ACCOUNT;
    act(() => emitEvent("workspaceNavigate", {}));
    await settle();
    expect(accountLandings()).toBe(1);

    act(() => emitEvent("workspaceNavigate", {}));
    await settle();
    expect(accountLandings()).toBe(1);
  });

  it("事件抢在挂载那次取之前到：两次取只有一次拿得到，落地一次", async () => {
    pendingInRust = ACCOUNT;
    render(workspace(0));
    // 监听已挂上、挂载那次取还没回来：此刻送达的事件先取走。
    act(() => emitEvent("workspaceNavigate", {}));
    await settle();
    expect(accountLandings()).toBe(1);
    expect(pendingInRust).toBeNull();
  });
});
