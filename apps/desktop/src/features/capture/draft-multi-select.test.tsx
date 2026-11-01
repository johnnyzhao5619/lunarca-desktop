// @vitest-environment happy-dom
/**
 * 速记清单的多选（PRD-06-059）。
 *
 * 真机 2026-09-09：接上共享行选之后，Cmd 加选与 Shift 连选在屏幕上「毫无反应」。两处根因
 * 都不在这份视图里——一是选中态外观按类名各写一份、漏了这条清单（见
 * `components/selected-state-guard.test.ts`），二是可见行第一帧为空时原语把当前那条剪掉了
 * （见 `workspace/note-selection-keyboard.test.tsx`）。这里真渲染一份清单、真点，钉住
 * 修完之后这条清单与另外五条讲的是同一种话。
 */

import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { ContextMenuHost } from "../../components/ContextMenuHost";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { DraftInbox } from "./DraftInbox";
import { useNoteSelection } from "../workspace/use-note-selection";
import type { DraftNoteDto } from "../../ipc/bindings";

function note(path: string, title: string): DraftNoteDto {
  return {
    path,
    title,
    preview: "",
    modified_ms: 1,
    quick_note: true,
    refined: false,
    has_media: false,
    duration_ms: 0,
    stale: false,
    can_merge: true,
    pinned_to_desktop: false,
    todo_open: 0,
    todo_total: 0,
  } as DraftNoteDto;
}

const NOTES = [
  note("QuickNotes/一.md", "第一条"),
  note("QuickNotes/二.md", "第二条"),
  note("QuickNotes/三.md", "第三条"),
];

function Harness() {
  const [selectedPath, setSelectedPath] = useState<string | null>("QuickNotes/一.md");
  const [rows, setRows] = useState<readonly string[]>([]);
  const selection = useNoteSelection({ current: selectedPath, visible: rows });
  return (
    <DraftInbox
      notes={NOTES}
      loadState={{ loading: false, failed: false, exhausted: true }}
      total={NOTES.length}
      filter="all"
      onFilterChange={() => {}}
      query=""
      onQueryChange={() => {}}
      folders={{ rootDir: undefined, dirs: [] } as never}
      selection={selection}
      onVisibleRows={setRows}
      selectedPath={selectedPath}
      onSelect={setSelectedPath}
      onOpenInNotes={() => {}}
      onSettle={() => {}}
      onDistill={() => {}}
      onCapture={async () => true}
      onVoiceCapture={() => {}}
        captureOpenRequest={0}
      voiceShortcut=""
      textShortcut=""
      onItemContextMenu={() => {}}
    />
  );
}

function mount() {
  cleanup();
  render(
    withI18n(
      <ContextMenuHost>
        <Harness />
      </ContextMenuHost>,
    ),
  );
}

/** 此刻被选中的那几行（按屏幕顺序）。 */
function selected(): string[] {
  return screen
    .getAllByRole("option")
    .filter((row) => row.getAttribute("aria-selected") === "true")
    .map((row) => row.querySelector(".luna-inbox__row-title")?.textContent ?? "");
}

/** 此刻正开着的那一条——与"选中的这一批"是两个问题。 */
function current(): string[] {
  return screen
    .getAllByRole("option")
    .filter((row) => row.hasAttribute("aria-current"))
    .map((row) => row.querySelector(".luna-inbox__row-title")?.textContent ?? "");
}

function row(title: string): HTMLElement {
  const found = screen
    .getAllByRole("option")
    .find((element) => element.querySelector(".luna-inbox__row-title")?.textContent === title);
  if (found === undefined) {
    throw new Error(`没有这一行：${title}`);
  }
  return found;
}

afterEach(cleanup);

describe("速记清单的多选", () => {
  it("挂载后当前那条就在选择集里（可见行是后一帧才报上来的）", () => {
    mount();
    expect(selected()).toEqual(["第一条"]);
  });

  it("Cmd 加选把第二条也选上，且不切换主区打开的那一篇", () => {
    mount();
    fireEvent.click(row("第二条"), { metaKey: true });
    expect(selected()).toEqual(["第一条", "第二条"]);
    expect(current()).toEqual(["第一条"]);
  });

  it("Shift 连选一段", () => {
    mount();
    fireEvent.click(row("第三条"), { shiftKey: true });
    expect(selected()).toEqual(["第一条", "第二条", "第三条"]);
  });

  it("无修饰键的单击照旧是「打开这一条」", () => {
    mount();
    fireEvent.click(row("第二条"));
    expect(current()).toEqual(["第二条"]);
    expect(selected()).toEqual(["第二条"]);
  });
});
