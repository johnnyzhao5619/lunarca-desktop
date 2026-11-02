// @vitest-environment happy-dom
/**
 * 高频广播不重渲染整个工作区（2026-10-15 真机卡顿）。
 *
 * 资源占用每约 1.7 秒一推、同步每一轮发好几次总览、任务运行时进度一拍一推、录音中计时
 * 每秒一走——此前这些都是 `WorkspaceApp` 的 state，于是每一拍都把编辑器、回放、知识库页
 * 连同整个工作区重渲染一遍。这里用真实的 `WorkspaceApp` 加三只探针把它钉住：
 *
 * - `root`：活动栏（工作区根每渲染一次它就渲染一次——它不 memo，props 每次都是新的）；
 * - `editor` / `playback`：主区内容（编辑器、回放视图）。
 *
 * 每条广播都同时断言**叶子确实更新了**（底栏 / 顶栏那一颗的文字或角标变了），否则「没重
 * 渲染」可能只是「订阅断了」。
 *
 * 对话流式回答同理（D1）：会话由工作区根持有（主侧栏历史与主区对话共用一份），一个 token
 * 换一次消息列表；回答流着的时候离开对话页去看笔记 / 录音，编辑器与回放也不能跟着一拍一渲。
 */
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onCloseRequested: () => Promise.resolve(() => {}),
    startDragging: () => Promise.resolve(),
  }),
}));
// 对话一轮的流式通道：替身只留 `onmessage`，用例经它逐个投递 token。
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));

const probe = vi.hoisted(() => ({ root: 0, editor: 0, playback: 0 }));
vi.mock("../features/workspace/WorkspacePrimaryRail", () => ({
  WorkspacePrimaryRail: () => {
    probe.root += 1;
    return null;
  },
}));
vi.mock("../features/editor/VaultEditor", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../features/editor/VaultEditor")>()),
  VaultEditor: () => {
    probe.editor += 1;
    return null;
  },
}));
vi.mock("../features/playback/SyncPlaybackView", () => ({
  SyncPlaybackView: () => {
    probe.playback += 1;
    return null;
  },
}));

import type {
  ProcTaskDto,
  RecordingState,
  SyncOverviewDto,
  VaultSyncDto,
} from "../ipc/bindings";
import { emitEvent, resetBindingsDouble, setCommandReply } from "../ipc/bindings-test-double";
import { t, withI18n } from "../i18n/i18n-test-fixture";
import { ConfirmHost } from "../components/ConfirmHost";
import { ContextMenuHost } from "../components/ContextMenuHost";
import { PromptHost } from "../components/PromptHost";
import { goTo } from "../lib/navigate";
import { WorkspaceApp } from "./WorkspaceApp";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});
beforeEach(() => {
  resetBindingsDouble();
  // 一方应用清单（ADR-165）：普通值命令，替身不会自己给出数组。
  setCommandReply("appsList", []);
  probe.root = 0;
  probe.editor = 0;
  probe.playback = 0;
  setCommandReply("processingQueueSnapshot", []);
  setCommandReply("listSettings", { status: "ok", data: [] });
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
  for (const name of ["vaultLinkPending", "externalRenamePrompt", "takePendingNavigation"]) {
    setCommandReply(name, null);
  }
  setCommandReply("recordingState", recording("idle"));
  // 录音空间的分组：归档目录与其下的子目录。
  setCommandReply("noteTypeArchiveDir", { status: "ok", data: "Recordings" });
  setCommandReply("vaultListDirs", { status: "ok", data: [] });
  // 对话页的取数：本机目录、执行路径、角色预设。
  setCommandReply("listModels", { status: "ok", data: [] });
  setCommandReply("computeStatus", unavailable);
  setCommandReply("listChatPresets", []);
  stream.channel = null;
  setCommandReply("chatSend", (_scope: unknown, _content: unknown, channel: unknown) => {
    stream.channel = channel as StreamChannel;
    return { status: "ok", data: null };
  });
});

interface StreamChannel {
  onmessage: ((event: unknown) => void) | null;
}

/** 最近一轮对话的流式通道（`chatSend` 收到的那一个）。 */
const stream: { channel: StreamChannel | null } = { channel: null };

function vault(overrides: Partial<VaultSyncDto> = {}): VaultSyncDto {
  return {
    path: "/v",
    display_path: "~/v",
    name: "v",
    is_open: true,
    phase: "idle",
    paused: false,
    remote_vault_id: "remote-v",
    last_synced_ms: 1,
    last_error: null,
    conflict_copies: 0,
    held_deletions: 0,
    too_large: [],
    local_bytes: 2048,
    cloud_bytes: null,
    bound_account: null,
    ...overrides,
  };
}

function overview(vaults: VaultSyncDto[]): SyncOverviewDto {
  return {
    gate: "ready",
    paused: null,
    storage: null,
    read_only: null,
    large_paused: null,
    account_frozen: false,
    vaults,
  };
}

function running(id: string, noteRel: string, done: number): ProcTaskDto {
  return {
    id,
    kind: "merge",
    label: id,
    note_rel: noteRel,
    status: { state: "running", data: { step: "transcribing", done, total: 100 } },
    executed_by: null,
    queue: null,
    restored: false,
  } as ProcTaskDto;
}

function recording(status: RecordingState["status"]): RecordingState {
  return {
    status,
    purpose: "record",
    session_id: status === "idle" ? null : "s1",
    note_rel: null,
    elapsed_ms: 0,
    message_key: null,
    message_detail: null,
  };
}

/** 两种主区：笔记（编辑器）与录音（回放视图）——各自都不能被广播带着重渲染。 */
const SURFACES = [
  { name: "笔记 · 编辑器", content: () => probe.editor, enter: () => {} },
  {
    name: "录音 · 回放视图",
    content: () => probe.playback,
    enter: () => goTo("recording://Recordings/a.md"),
  },
] as const;

async function mountWorkspace(enter: () => void) {
  const view = render(
    withI18n(
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
              initialVault: { open: true, root: "/v", missing_root: null, missing_in_cloud: false },
              launchOverview: false,
            }),
          ),
        ),
      ),
    ),
  );
  // 启动取数（库、设置、队列快照、录音状态……）全部落地。
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 50));
  });
  act(() => enter());
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
  return view;
}

/** 此刻三只探针的读数；之后再读一次比对，相等 = 工作区根与主区都没被带着渲染。 */
function snapshot(content: () => number) {
  // 主区探针必须真的挂着，否则「没重渲染」只是「压根没渲染」。
  expect(content()).toBeGreaterThan(0);
  return { root: probe.root, content: content() };
}

describe.each(SURFACES)("高频广播只重渲染底栏 / 顶栏那一颗（$name）", ({ content, enter }) => {
  it("资源占用：一拍只换 CPU 那颗按钮，工作区根与主区不动", async () => {
    const view = await mountWorkspace(enter);
    const before = snapshot(content);
    const button = () =>
      view.container.querySelector<HTMLButtonElement>(".luna-workspace__statusbar-res");

    act(() => emitEvent("resourceUsage", { cpu_percent: 12.3, gpu_percent: null }));
    expect(button()?.getAttribute("aria-label")).toBe(t("workspace:statusbar.resourceLabel", { cpu: 12 }));
    act(() => emitEvent("resourceUsage", { cpu_percent: 40.2, gpu_percent: 7.6 }));
    expect(button()?.getAttribute("aria-label")).toBe(
      t("workspace:statusbar.resourceLabelGpu", { cpu: 40, gpu: 8 }),
    );
    expect(snapshot(content)).toEqual(before);
  });

  it("同步总览：一轮里只有每库细节在变（字节、完成时刻）时谁都不动；该说的话变了只换底栏那颗", async () => {
    const view = await mountWorkspace(enter);
    const label = () =>
      view.container.querySelector(".luna-workspace__statusbar-sync-btn")?.textContent ?? null;

    act(() => emitEvent("syncStatus", overview([vault()])));
    expect(label()).toBe(t("workspace:statusbar.sync.synced"));
    const before = snapshot(content);

    // 同一句「已同步」：底栏取的那一位、文件树取的「开没开同步」都没变。
    act(() => emitEvent("syncStatus", overview([vault({ local_bytes: 4096, last_synced_ms: 2 })])));
    act(() => emitEvent("syncStatus", overview([vault({ cloud_bytes: 9000, last_synced_ms: 3 })])));
    expect(label()).toBe(t("workspace:statusbar.sync.synced"));
    // 换一句话：底栏那颗换字，根与主区照样不动。
    act(() => emitEvent("syncStatus", overview([vault({ phase: "syncing" })])));
    expect(label()).toBe(t("workspace:statusbar.sync.syncing"));
    expect(snapshot(content)).toEqual(before);
  });

  it("处理队列：进度一拍一推不动根与主区，顶栏入口的角标照常跟", async () => {
    const view = await mountWorkspace(enter);
    const badge = () =>
      view.container.querySelector(".luna-proc-queue-btn__badge")?.textContent ?? null;

    // 「有没有在跑的」从无到有：根取的那一位翻转，允许它渲染这一次。
    act(() => emitEvent("processingQueue", { items: [running("a", "Notes/x.md", 10)] }));
    expect(badge()).toBe("1");
    const before = snapshot(content);

    act(() => emitEvent("processingQueue", { items: [running("a", "Notes/x.md", 40)] }));
    act(() => emitEvent("processingQueue", { items: [running("a", "Notes/x.md", 80)] }));
    act(() =>
      emitEvent("processingQueue", {
        items: [running("a", "Notes/x.md", 90), running("b", "Notes/y.md", 0)],
      }),
    );
    expect(badge()).toBe("2");
    expect(snapshot(content)).toEqual(before);
  });

  it("录音计时：每秒一走只换底栏那颗，根只在录音状态转移时渲染", async () => {
    const view = await mountWorkspace(enter);
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    const label = () =>
      view.container.querySelector(".luna-workspace__statusbar-recording")?.getAttribute("aria-label") ??
      null;

    act(() => emitEvent("recordingStateChanged", recording("recording")));
    expect(label()).toBe(t("workspace:statusbar.recordingElapsed", { time: "0:00" }));
    const before = snapshot(content);

    act(() => vi.advanceTimersByTime(3000));
    expect(label()).toBe(t("workspace:statusbar.recordingElapsed", { time: "0:03" }));
    expect(snapshot(content)).toEqual(before);
  });
});

describe("工作区根仍跟着它真正要的那几位走", () => {
  it("录音状态转移、队列从闲到忙会渲染根（选择器不是把订阅掐断了）", async () => {
    await mountWorkspace(() => {});
    const rootBefore = probe.root;
    act(() => emitEvent("recordingStateChanged", recording("recording")));
    expect(probe.root).toBeGreaterThan(rootBefore);

    await waitFor(() => expect(probe.root).toBeGreaterThan(0));
    const afterRecording = probe.root;
    act(() => emitEvent("processingQueue", { items: [running("a", "Notes/x.md", 10)] }));
    expect(probe.root).toBeGreaterThan(afterRecording);
  });
});

/** 在对话页发一条消息，等到 `chatSend` 拿到流式通道并吐出第一个 token。 */
async function startStreaming(view: ReturnType<typeof render>) {
  const input = view.container.querySelector<HTMLTextAreaElement>(".luna-chat__composer textarea");
  expect(input).not.toBeNull();
  fireEvent.change(input!, { target: { value: "hi" } });
  fireEvent.submit(view.container.querySelector(".luna-chat__composer")!);
  await waitFor(() => expect(stream.channel?.onmessage).toBeTypeOf("function"));
  act(() => token("Hel"));
}

function token(text: string) {
  stream.channel!.onmessage!({ kind: "token", data: text });
}

/** 主区里那条正在流的回答此刻的文字。 */
function streamingAnswer(view: ReturnType<typeof render>): string | null {
  return (
    view.container.querySelector('.luna-chat-message[data-role="assistant"][data-status="running"]')
      ?.textContent ?? null
  );
}

describe("对话流式回答只重渲染对话视图", () => {
  it("对话页上：token 一个个到，回答在长，工作区根不动", async () => {
    const view = await mountWorkspace(() => goTo("chats://"));
    await startStreaming(view);
    expect(streamingAnswer(view)).toContain("Hel");
    const rootBefore = probe.root;

    act(() => token("lo"));
    act(() => token(" wor"));
    act(() => token("ld"));
    expect(streamingAnswer(view)).toContain("Hello world");
    expect(probe.root).toBe(rootBefore);
  });

  it.each([
    { name: "笔记 · 编辑器", content: () => probe.editor, leave: () => goTo("notes://") },
    {
      name: "录音 · 回放视图",
      content: () => probe.playback,
      leave: () => goTo("recording://Recordings/a.md"),
    },
  ])("回答流着时离开对话页（$name）：根与主区不随 token 渲染，回来时回答一字不少", async ({ content, leave }) => {
    const view = await mountWorkspace(() => goTo("chats://"));
    await startStreaming(view);
    act(() => leave());
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    expect(streamingAnswer(view)).toBeNull();
    const before = snapshot(content);

    act(() => token("lo"));
    act(() => token(" wor"));
    act(() => token("ld"));
    expect(snapshot(content)).toEqual(before);

    act(() => goTo("chats://"));
    await waitFor(() => expect(streamingAnswer(view)).toContain("Hello world"));
  });
});
