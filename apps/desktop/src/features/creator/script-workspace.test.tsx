// @vitest-environment happy-dom
/**
 * 脚本工作区（ADR-165 ⑦ / PRD-19-038）：工具栏（提词器、口述、AI 改稿、时长与语速）与检查器
 * （段落与时长、调段序、本项目引文、灵感）。分段、时长、校准、引文与套路都由 Rust 给，这里验
 * 呈现与发出去的命令 / 事件。这些行为此前在上下文面板「大纲」里，迁过来时逐条保住。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import {
  EDITOR_REVEAL_EVENT,
  SCRIPT_INSERT_EVENT,
  SCRIPT_RATE_CHANGED_EVENT,
  SCRIPT_REWRITE_EVENT,
  SCRIPT_SECTION_MOVE_EVENT,
  SCRIPT_BROLL_TOGGLE_EVENT,
  type ScriptBrollToggleDetail,
  type EditorRevealDetail,
  type ScriptInsertDetail,
  type ScriptRewriteDetail,
  type ScriptSectionMoveDetail,
} from "../../lib/app-events";
import type { ScriptOutlineDto } from "../../ipc/bindings";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import { ScriptInspector } from "./ScriptInspector";
import { ScriptToolbar } from "./ScriptToolbar";

const PATH = "Creator/P/P-脚本.md";

const SCRIPT: ScriptOutlineDto = {
  sections: [
    { heading: null, line: 1, cjk_chars: 60, words: 0, broll_blocks: 0, seconds: 15 },
    { heading: "开头", line: 5, cjk_chars: 240, words: 0, broll_blocks: 0, seconds: 60 },
    { heading: "结尾", line: 9, cjk_chars: 0, words: 150, broll_blocks: 1, seconds: 60 },
  ],
  shots: [],
  total_seconds: 135,
  cjk_per_minute: 240,
  words_per_minute: 150,
  calibrated: false,
  sample_minutes: 0,
  recordings: 0,
};

function listen<T>(name: string) {
  const received: T[] = [];
  const listener = (event: Event) => received.push((event as CustomEvent<T>).detail);
  window.addEventListener(name, listener);
  return { received, stop: () => window.removeEventListener(name, listener) };
}

function toolbar(outline: ScriptOutlineDto | null = SCRIPT) {
  return render(
    withI18n(createElement(ContextMenuHost, null, createElement(ScriptToolbar, { path: PATH, outline, onError: () => {} }))),
  );
}

function inspector(outline: ScriptOutlineDto | null = SCRIPT, onOpenNote = vi.fn()) {
  render(withI18n(createElement(ScriptInspector, { home: "Creator/P/P.md", path: PATH, outline, onOpenNote })));
  return onOpenNote;
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("creatorProjectQuotes", { status: "ok", data: [] });
  setCommandReply("creatorSwipeLibrary", { status: "ok", data: { groups: [], total: 0 } });
});
afterEach(cleanup);

describe("脚本工具栏", () => {
  it("报全篇约多久；点开是语速，未校准时给「用我的录音校准」，样本不够如实说还差多少、不通知重算", async () => {
    setCommandReply("creatorCalibrateSpeakingRate", { status: "ok", data: { kind: "not_enough", minutes: 0.4 } });
    const recompute = listen(SCRIPT_RATE_CHANGED_EVENT);
    toolbar();
    const total = screen.getByRole("button", { name: t("workspace:creator.script.totalApprox", { duration: "2:15" }) });
    expect(screen.queryByText(t("workspace:creator.script.rateDefault", { cjk: 240, words: 150 }))).toBeNull();
    fireEvent.click(total);
    expect(screen.getByText(t("workspace:creator.script.rateDefault", { cjk: 240, words: 150 }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.script.calibrate") }));
    expect(await screen.findByText(t("workspace:creator.script.notEnough", { minutes: 0.4 }))).toBeTruthy();
    expect(recompute.received).toHaveLength(0);
    recompute.stop();
  });

  it("校准过就报样本来源并给「重新校准 / 恢复默认」；校准成功通知编辑器重算", async () => {
    setCommandReply("creatorCalibrateSpeakingRate", {
      status: "ok",
      data: { kind: "calibrated", cjk_per_minute: 280, words_per_minute: null, sample_minutes: 3.5, recordings: 4 },
    });
    const recompute = listen(SCRIPT_RATE_CHANGED_EVENT);
    toolbar({ ...SCRIPT, calibrated: true, cjk_per_minute: 280, sample_minutes: 3.5, recordings: 4 });
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.script.total", { duration: "2:15" }) }));
    expect(screen.getByText(t("workspace:creator.script.sample", { recordings: 4, minutes: 3.5 }))).toBeTruthy();
    expect(screen.getByRole("button", { name: t("workspace:creator.script.resetRate") })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.script.recalibrate") }));
    await waitFor(() => expect(recompute.received).toHaveLength(1));
    recompute.stop();
  });

  it("「提词器」用这一篇打开提词器", () => {
    setCommandReply("teleprompterOpen", { status: "ok", data: null });
    toolbar();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.script.teleprompter") }));
    expect(commandMock("teleprompterOpen")).toHaveBeenCalledWith(PATH, false);
  });

  it("「AI 改稿」列出四个动作，选一个就把它交给编辑器（与选区菜单同一组）", async () => {
    const rewrites = listen<ScriptRewriteDetail>(SCRIPT_REWRITE_EVENT);
    toolbar();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.script.rewrite") }));
    const menu = await screen.findByRole("menu");
    for (const key of ["tighten", "hook", "spoken", "dictation"]) {
      expect(within(menu).getByRole("menuitem", { name: t(`editor:floating.script.${key}`) })).toBeTruthy();
    }
    fireEvent.click(within(menu).getByRole("menuitem", { name: t("editor:floating.script.hook") }));
    expect(rewrites.received).toEqual([{ path: PATH, action: "script_hook" }]);
    rewrites.stop();
  });

  it("时长还没算出来时只是不显示时长，工具照常可用（没有标题的脚本也能开提词器、口述、改稿）", () => {
    toolbar(null);
    expect(screen.getByRole("button", { name: t("workspace:creator.script.teleprompter") })).toBeTruthy();
    expect(screen.getByRole("button", { name: t("workspace:creator.dictation.start") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /2:15/ })).toBeNull();
  });
});

describe("脚本检查器：段落", () => {
  it("每段约念多久，开场单独一行；只有一个可搬的段以上才提示怎么调序", () => {
    inspector();
    expect(screen.getByText(t("workspace:creator.script.opening"))).toBeTruthy();
    expect(screen.getByText("0:15")).toBeTruthy();
    expect(screen.getAllByText("1:00")).toHaveLength(2);
    expect(
      screen.getByText(t("workspace:creator.script.reorderHint", { up: formatShortcut("alt+up"), down: formatShortcut("alt+down") })),
    ).toBeTruthy();
  });

  it("⌥↑ / ⌥↓ 把这一段与相邻段对调；到头了、不带 ⌥、开场都不动", () => {
    const moves = listen<ScriptSectionMoveDetail>(SCRIPT_SECTION_MOVE_EVENT);
    inspector();
    const opening = screen.getByText("开头").closest("button")!;
    const ending = screen.getByText("结尾").closest("button")!;
    fireEvent.keyDown(opening, { key: "ArrowUp", altKey: true });
    fireEvent.keyDown(ending, { key: "ArrowDown", altKey: true });
    fireEvent.keyDown(opening, { key: "ArrowDown" });
    expect(moves.received).toEqual([]);
    fireEvent.keyDown(opening, { key: "ArrowDown", altKey: true });
    fireEvent.keyDown(ending, { key: "ArrowUp", altKey: true });
    expect(moves.received).toEqual([
      { path: PATH, fromLine: 5, beforeLine: null },
      { path: PATH, fromLine: 9, beforeLine: 5 },
    ]);
    expect(screen.getByText(t("workspace:creator.script.opening")).closest("button")?.getAttribute("aria-keyshortcuts")).toBeNull();
    moves.stop();
  });

  it("拖到某一段上 = 插到它之前；拖到开场上 = 插到第一段之前", () => {
    const moves = listen<ScriptSectionMoveDetail>(SCRIPT_SECTION_MOVE_EVENT);
    inspector();
    const first = screen.getByText("开头").closest("li")!;
    const opening = screen.getByText(t("workspace:creator.script.opening")).closest("li")!;
    const ending = screen.getByText("结尾").closest("button")!;
    const dropAt = vi.spyOn(document, "elementFromPoint");
    const dragTo = (target: Element) => {
      dropAt.mockReturnValue(target);
      fireEvent.pointerDown(ending, { clientX: 10, clientY: 100 });
      fireEvent.pointerMove(window, { clientX: 10, clientY: 40 });
      fireEvent.pointerUp(window, { clientX: 10, clientY: 40 });
    };
    dragTo(first);
    expect(moves.received[moves.received.length - 1]).toEqual({ path: PATH, fromLine: 9, beforeLine: 5 });
    dragTo(opening);
    expect(moves.received[moves.received.length - 1]).toEqual({ path: PATH, fromLine: 9, beforeLine: 5 });
    dropAt.mockRestore();
    moves.stop();
  });

  it("点可搬的段：跳过去但焦点留在这一行；点开场把焦点交给编辑器", () => {
    const reveals = listen<EditorRevealDetail>(EDITOR_REVEAL_EVENT);
    inspector();
    const section = screen.getByText("开头").closest("button")!;
    fireEvent.mouseDown(section);
    expect(document.activeElement).toBe(section);
    fireEvent.click(section);
    fireEvent.click(screen.getByText(t("workspace:creator.script.opening")).closest("button")!);
    expect(reveals.received.map((detail) => [detail.line, detail.focusEditor])).toEqual([
      [5, false],
      [1, true],
    ]);
    reveals.stop();
  });

  it("没有二级标题：说清怎么分段", () => {
    inspector({ ...SCRIPT, sections: [{ heading: null, line: 1, cjk_chars: 10, words: 0, broll_blocks: 0, seconds: 3 }] });
    expect(screen.getByText(t("workspace:creator.script.noSections"))).toBeTruthy();
  });
});

describe("脚本检查器：引文与灵感", () => {
  it("列出本项目的引文卡片，点一张就把它的嵌入插到脚本光标处", async () => {
    setCommandReply("creatorProjectQuotes", {
      status: "ok",
      data: [{ research_path: "Creator/P/P-研究.md", text: "带宽比算力先撞墙 —— 访谈 · 4:12", embed: "![[P-研究#^q-abc123]]" }],
    });
    const inserts = listen<ScriptInsertDetail>(SCRIPT_INSERT_EVENT);
    inspector();
    fireEvent.click(await screen.findByText("带宽比算力先撞墙 —— 访谈 · 4:12"));
    expect(inserts.received).toEqual([{ path: PATH, text: "![[P-研究#^q-abc123]]" }]);
    expect(commandMock("creatorProjectQuotes")).toHaveBeenCalledWith(PATH);
    inserts.stop();
  });

  it("还没有引文时说清怎么摘", async () => {
    inspector();
    expect(await screen.findByText(t("workspace:creator.quotes.none"))).toBeTruthy();
  });

  it("灵感按套路分组，每组列前几条；点一条交给项目打开", async () => {
    setCommandReply("creatorSwipeLibrary", {
      status: "ok",
      data: {
        groups: [
          {
            name: "反常识",
            items: [
              { path: "Creator/Swipe/a.md", title: "a", modified_ms: 1, why: null },
              { path: "Creator/Swipe/b.md", title: "b", modified_ms: 1, why: null },
              { path: "Creator/Swipe/c.md", title: "c", modified_ms: 1, why: null },
              { path: "Creator/Swipe/d.md", title: "d", modified_ms: 1, why: null },
            ],
          },
        ],
        total: 4,
      },
    });
    const onOpenNote = inspector();
    expect(await screen.findByText("反常识")).toBeTruthy();
    expect(screen.queryByText("d")).toBeNull();
    fireEvent.click(screen.getByText("b"));
    expect(onOpenNote).toHaveBeenCalledWith("Creator/Swipe/b.md");
  });

  it("镜头清单：没有 broll 块不出现；勾选交给编辑器改围栏行，导出 CSV 用的是这一份镜头", async () => {
    inspector();
    expect(screen.queryByText(t("workspace:creator.shots.export"))).toBeNull();
    cleanup();
    const shots = [
      { section: null, line: 3, text: "街景", done: true },
      { section: "结尾", line: 11, text: "键盘特写\n屏幕录制", done: false },
    ];
    setCommandReply("creatorExportBroll", { status: "ok", data: "Creator/P/P-镜头清单.csv" });
    setCommandReply("revealVaultPath", { status: "ok", data: null });
    inspector({ ...SCRIPT, shots });
    expect(screen.getByRole("heading", { name: t("workspace:creator.shots.title", { done: 1, total: 2 }) })).toBeTruthy();
    const toggles = listen<ScriptBrollToggleDetail>(SCRIPT_BROLL_TOGGLE_EVENT);
    fireEvent.click(screen.getByRole("checkbox", { name: t("workspace:creator.shots.toggle", { shot: "键盘特写" }) }));
    toggles.stop();
    expect(toggles.received).toEqual([{ path: PATH, line: 11, done: true }]);
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.shots.export") }));
    await waitFor(() => expect(commandMock("creatorExportBroll")).toHaveBeenCalledWith("Creator/P/P.md", shots));
    expect(await screen.findByText(t("workspace:creator.shots.exported", { file: "P-镜头清单.csv" }))).toBeTruthy();
    expect(commandMock("revealVaultPath")).toHaveBeenCalledWith("Creator/P/P-镜头清单.csv");
  });
});
