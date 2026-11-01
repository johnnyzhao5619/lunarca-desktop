// @vitest-environment happy-dom
/// <reference types="vite/client" />
/**
 * 五类创建动作落到真实命令的那一端（ADR-087 / PRD-07-069 / TDD-07 §4.2c）。
 *
 * 与隔壁 `create-actions.test.ts` 一起构成完整的一条链，两者按 `CreateActionContext`
 * 这个边界分工，谁都不覆盖谁：
 *   - `create-actions.test.ts`：注册表 → 上下文能力（五个 id 不重不漏、各调各的、终态原样回传）
 *   - 本文件：上下文能力 → 真实命令
 * 只守前半段，宿主里悄悄断掉一条接线不会让任何用例变红——用户看到的是那个入口按下去
 * 什么都不发生。「重组页面」最容易丢的正是它们中的某一条：丢了不报错，只是从此不存在。
 *
 * 批次 E：能真跑的就真跑，跑不动的说清楚为什么**。
 * - 「开始录音」「导入资料」的接线各自长在一个可独立挂载的 hook 上 → 真实调用：
 *   跑一遍 `start()`，断言真的按顺序发出了那几条命令、参数是那几个。
 * - 「文字速记」「新建笔记」「语音速记」的接线长在组装根 `app/WorkspaceApp.tsx` 上
 *   （两千余行、几十个 feature 的组合点）。把它整棵挂起来才能按到那几个入口，代价与
 *   收益不成比例，且组装根正被并行批次改动。故这三条仍按源码断言，并在此写明它
 *   证明的是"这条调用还在源码里"，不是"点下去真的发生了"——比对的三条链路则各自在
 *   `create-actions.test.ts`（注册表 → 能力）与 `hud-preset.test.ts`（HUD 落点）里
 *   有真实行为覆盖。
 */

import { createElement, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

// 录音 hook 用 Tauri 的 `Channel` 收事件与电平；本文件只看它发了哪几条命令。
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((message: unknown) => void) | null = null;
  },
}));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { commands } from "../../ipc/bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { useRecording } from "../recording/use-recording";
import { useTranscribeImport } from "../transcribe/use-transcribe-import";

const workspaceApps = import.meta.glob("../../app/WorkspaceApp.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const WORKSPACE_APP = Object.values(workspaceApps)[0] ?? "";

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("recordingState", { status: "idle", session_id: null, note_rel: null });
  setCommandReply("listAudioInputDevices", []);
  setCommandReply("listSettings", { status: "ok", data: [] });
});
afterEach(cleanup);

describe("start_recording：先建 `recording` 归档笔记，再启动会话", () => {
  it("顺序不能反——先有归档笔记，`recording_start` 才有地方写", async () => {
    setCommandReply("createNote", { status: "ok", data: { path: "资料/录音/2026-08-26.md" } });
    setCommandReply("recordingStart", { status: "ok", data: null });

    const { result } = renderHook(() => useRecording());
    await act(async () => {
      await result.current.actions.start();
    });

    expect(commandMock("createNote")).toHaveBeenCalledWith("recording");
    const [sessionId, noteRel] = commandMock("recordingStart").mock.calls[0];
    expect(typeof sessionId).toBe("string");
    expect(noteRel, "启动会话时必须带上那篇归档笔记").toBe("资料/录音/2026-08-26.md");
    expect(
      commandMock("createNote").mock.invocationCallOrder[0],
      "建笔记必须在启动之前",
    ).toBeLessThan(commandMock("recordingStart").mock.invocationCallOrder[0]);
  });

  it("建不出归档笔记就**不启动会话**，并如实报错", async () => {
    // 反过来（照样启动）的表现是：录完一段，落盘时才发现没地方写，音频已经没了。
    setCommandReply("createNote", { status: "error", error: "VAULT_UNAVAILABLE" });

    const { result } = renderHook(() => useRecording());
    await act(async () => {
      await result.current.actions.start();
    });

    expect(commandMock("recordingStart")).not.toHaveBeenCalled();
    await waitFor(() => expect(result.current.state.status).toBe("error"));
    expect(result.current.state.message_key).toBe("recording:error.note_create_failed");
  });
});

describe("import_material：导入走统一处理队列的既有导入命令", () => {
  // 选与入队之间过一道任务前预估（PRD-13-040 ②），那道门用全应用唯一的确认宿主。
  const wrapper = ({ children }: { children: ReactNode }) => createElement(ConfirmHost, null, children);

  it("选好文件后入队，并把 Rust 回的入队件数原样交回调用方", async () => {
    setCommandReply("pickMediaFiles", { status: "ok", data: ["/a.m4a", "/b.mp4", "/c.wav"] });
    setCommandReply("taskEstimate", null);
    setCommandReply("importMediaPaths", { status: "ok", data: 3 });
    const { result } = renderHook(() => useTranscribeImport(() => {}), { wrapper });

    let queued = 0;
    await act(async () => {
      queued = await result.current.start();
    });

    expect(commandMock("pickMediaFiles")).toHaveBeenCalledTimes(1);
    expect(commandMock("taskEstimate")).toHaveBeenCalledWith({
      kind: "import_media",
      paths: ["/a.m4a", "/b.mp4", "/c.wav"],
    });
    expect(commandMock("importMediaPaths")).toHaveBeenCalledWith(["/a.m4a", "/b.mp4", "/c.wav"]);
    expect(queued, "件数是入口到达率之外的另一半事实").toBe(3);
  });

  it("「导入资料」（ADR-164 ⑩）：音视频与文档同一个选择框，整批交给 Rust 分流", async () => {
    setCommandReply("pickMaterialFiles", { status: "ok", data: ["/a.m4a", "/提纲.docx", "/论文.pdf"] });
    setCommandReply("taskEstimate", null);
    setCommandReply("importMaterialPaths", { status: "ok", data: 3 });
    const { result } = renderHook(
      () => useTranscribeImport(() => {}, commands.importMaterialPaths, commands.pickMaterialFiles),
      { wrapper },
    );

    let queued = 0;
    await act(async () => {
      queued = await result.current.start();
    });

    expect(commandMock("pickMediaFiles")).not.toHaveBeenCalled();
    expect(commandMock("importMaterialPaths")).toHaveBeenCalledWith(["/a.m4a", "/提纲.docx", "/论文.pdf"]);
    expect(commandMock("importMediaPaths")).not.toHaveBeenCalled();
    expect(queued).toBe(3);
  });

  it("取消选择：不估、不入队，计 0 件", async () => {
    setCommandReply("pickMediaFiles", { status: "ok", data: [] });
    const { result } = renderHook(() => useTranscribeImport(() => {}), { wrapper });
    let queued = -1;
    await act(async () => {
      queued = await result.current.start();
    });
    expect(queued).toBe(0);
    expect(commandMock("taskEstimate")).not.toHaveBeenCalled();
    expect(commandMock("importMediaPaths")).not.toHaveBeenCalled();
  });

  it("失败经 `onError` 回流且计 0 件——不能把一次失败伪装成「导入了 0 个文件」", async () => {
    setCommandReply("pickMediaFiles", { status: "error", error: "MODEL_MISSING" });
    const onError = vi.fn();
    const { result } = renderHook(() => useTranscribeImport(onError), { wrapper });

    let queued = -1;
    await act(async () => {
      queued = await result.current.start();
    });

    expect(onError).toHaveBeenCalledWith("MODEL_MISSING");
    expect(queued).toBe(0);
  });

  it("在途时不重复唤起（对话框已经开着，再开一个是两份选择）", async () => {
    let release: (value: unknown) => void = () => {};
    setCommandReply("pickMediaFiles", () => new Promise((resolve) => (release = resolve)));
    const { result } = renderHook(() => useTranscribeImport(() => {}), { wrapper });

    let first: Promise<number> = Promise.resolve(0);
    act(() => {
      first = result.current.start();
    });
    await waitFor(() => expect(result.current.importing).toBe(true));
    await act(async () => {
      expect(await result.current.start()).toBe(0);
    });
    expect(commandMock("pickMediaFiles")).toHaveBeenCalledTimes(1);

    await act(async () => {
      release({ status: "ok", data: [] });
      await first;
    });
  });
});

describe("组装根上的三条接线（源码断言，理由见文件头）", () => {
  it("扫描面非空", () => {
    expect(WORKSPACE_APP.length).toBeGreaterThan(200);
  });

  it("text_quick_note：文字捕获恒传 `drafts` 目的地，落点判定在 Rust", () => {
    expect(WORKSPACE_APP).toContain('commands.createNoteFromText(text, "drafts")');
  });

  it("text_quick_note：入口要**显式聚焦**捕获框，不靠挂载副作用", () => {
    // 2026-09-09 评审：这条接线此前只有 `go("quick_notes")`，聚焦全靠 `CaptureBox` 的
    // `autoFocus`——那只在这一格第一次挂载时生效。用户已经站在「速记」时再点这一
    // 项，`go()` 是空操作、组件不重挂载，于是屏幕上什么都不发生，而指标那一侧照记一次
    // 「到达」。这是「丢了不报错，只是从此不存在」的又一个实例，故钉在这里。
    // 2026-10-08 起文字捕获默认收起（R1）：入口发一次展开请求，由捕获胶囊展开并聚焦
    // （`CaptureBox.openRequest`，行为由 capture-ux.test.ts 真实渲染断言）。
    expect(WORKSPACE_APP).toContain("setCaptureOpenRequest((request) => request + 1)");
    expect(WORKSPACE_APP).toContain("captureOpenRequest={captureOpenRequest}");
  });

  it("text_quick_note：库没打开时终态是 `rejected`，不是恒真的「到达」", () => {
    // 恒记 arrived 会让入口到达率只可能是 100%——一个恒为 1 的比率比没有这条指标
    // 更糟：它看起来在回答问题，实际什么都没测（`create-actions.tsx` 文件头同一条纪律）。
    expect(WORKSPACE_APP).toContain('vaultOpen === false ? "rejected" : "arrived"');
  });

  it("new_note：新建正式笔记走 `create_note('note')`", () => {
    expect(WORKSPACE_APP).toContain('commands.createNote("note")');
  });

  it("voice_quick_note：语音速记入口显式把本次会话预设成速记（PRD-02-020）", () => {
    expect(WORKSPACE_APP).toContain('showVoiceInputHud("quick_note")');
  });
});

describe("HUD 落点预设没有缺省值（TDD-07 §2.3）", () => {
  // 规格明写"preset 为必填，无缺省参数兼容"。理由是调用点可读性：留一个
  // `= "current"`，"忘了传"与"就是要 current"从调用点看长得一模一样——而这两者的区别
  // 正是 PRD-07-069 的全部内容。真机 2026-08-26 栽的就是这一下：速记捕获框上的 🎤
  // 靠着那个缺省值拿到了「注入光标」，而它所在的那一格标着「速记」。
  //
  // 这条只能按源码断言：缺省参数是语法，运行时看不见（传了 `undefined` 与不传在
  // 函数里毫无区别），TypeScript 也不会因为多一个缺省值而报错。
  const FRONTEND = import.meta.glob("/src/**/*.{ts,tsx}", {
    eager: true,
    import: "default",
    query: "?raw",
  }) as Record<string, string>;

  it("扫描面覆盖整个前端源码树", () => {
    expect(Object.keys(FRONTEND).length).toBeGreaterThan(100);
  });

  it("声明处不带缺省值", () => {
    expect(WORKSPACE_APP).not.toMatch(/preset:\s*VoiceInputHudPresetDto\s*=/);
  });

  it("没有任何调用点空着手唤起 HUD", () => {
    const offenders = Object.entries(FRONTEND)
      .filter(([path]) => !path.endsWith(".test.ts") && !path.endsWith(".test.tsx"))
      .filter(([, source]) =>
        /\b(showVoiceInputHud|voiceInputShowHud)\s*\(\s*\)/.test(source),
      )
      .map(([path]) => path);
    expect(offenders, "空参唤起 = 落点由缺省值决定，入口标注随即成为一句空话").toEqual([]);
  });
});
