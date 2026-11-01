// @vitest-environment happy-dom
/**
 * AI 对话历史清单的多选（PRD-06-059）。
 *
 * 真机 2026-09-09：这条清单是七个空间里唯一连加选都没有的——行是一个 `role="button"` 的
 * div，既没有 `option` 角色，也没有键盘契约，右键菜单永远只作用于一条。这里真渲染一份
 * 清单、真点，钉住它现在与另外五条讲的是同一种话。
 *
 * 平台那一半（WKWebView 点按钮不给焦点）测不到，见 `note-selection-keyboard.test.tsx`。
 */

import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { ContextMenuHost } from "../../components/ContextMenuHost";
import { PromptHost } from "../../components/PromptHost";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { ChatHistoryList } from "./ChatHistoryList";
import { useNoteSelection } from "../workspace/use-note-selection";
import type { ChatHistoryEntryDto } from "../../ipc/bindings";

const ENTRIES: ChatHistoryEntryDto[] = [
  { path: "Chats/一.md", title: "第一场", modified_ms: 3 },
  { path: "Chats/二.md", title: "第二场", modified_ms: 2 },
  { path: "Chats/三.md", title: "第三场", modified_ms: 1 },
];

const deleted: string[][] = [];
/** 右键菜单收到的那一批（菜单本身由宿主装配，这里只钉"作用对象是谁"）。 */
const menuTargets: string[][] = [];

function Harness({ total = ENTRIES.length }: { total?: number } = {}) {
  const [active, setActive] = useState<string | null>("Chats/一.md");
  const [rows, setRows] = useState<readonly string[]>([]);
  const selection = useNoteSelection({ current: active, visible: rows });
  return (
    <ChatHistoryList
      entries={ENTRIES}
      loadState={{ loading: false, failed: false, exhausted: true }}
      total={total}
      activePath={active}
      canOpenInNotes={false}
      onNewChat={() => {}}
      onOpen={setActive}
      onDelete={(paths) => deleted.push([...paths])}
      onItemContextMenu={(_event, _entry, targets) => menuTargets.push([...targets])}
      selection={selection}
      onVisibleRows={setRows}
    />
  );
}

function mount(total?: number) {
  deleted.length = 0;
  menuTargets.length = 0;
  cleanup();
  render(
    withI18n(
      <ContextMenuHost>
        <PromptHost>
          <Harness total={total} />
        </PromptHost>
      </ContextMenuHost>,
    ),
  );
}

/** 此刻被选中的那几行（按屏幕顺序）。 */
function selected(): string[] {
  return screen
    .getAllByRole("option")
    .filter((row) => row.getAttribute("aria-selected") === "true")
    .map((row) => row.textContent ?? "");
}

/** 此刻正开着的那一条（工作面跟随它——它与"选中的这一批"是两个问题）。 */
function current(): string[] {
  return screen
    .getAllByRole("option")
    .filter((row) => row.hasAttribute("aria-current"))
    .map((row) => row.textContent ?? "");
}

/** 最近一次右键交出去的那一批（`Array.at` 不在本仓的 TS lib 里）。 */
function lastMenuTargets(): string[] {
  return menuTargets[menuTargets.length - 1] ?? [];
}

function row(title: string): HTMLElement {
  const found = screen
    .getAllByRole("option")
    .find((element) => (element.textContent ?? "").includes(title));
  if (found === undefined) {
    throw new Error(`没有这一行：${title}`);
  }
  return found;
}

describe("AI 对话历史清单", () => {
  it("页头徽标报的是全库总数，不是这一页有几行", () => {
    // 真机 2026-09-09 复盘：Rust 那头写死 100 条 top-N，而徽标写的是 `entries.length`——
    // 第 101 场对话既不在列表里，那个数字也从此停在 100。徽标承诺的是"一共有几场"。
    mount(137);
    expect(screen.getByText("137")).toBeDefined();
  });
});

describe("AI 对话历史的多选", () => {
  it("Cmd 加选把第二条也选上，且**不把正在进行的对话换掉**", () => {
    mount();
    expect(selected()).toEqual([expect.stringContaining("第一场")]);
    fireEvent.click(row("第二场"), { metaKey: true });
    expect(selected()).toHaveLength(2);
    // 加选一条就顺手载入它，等于每加选一条就把输入框里没发出去的那句冲掉。
    expect(current()).toEqual([expect.stringContaining("第一场")]);
  });

  it("Shift 连选一段", () => {
    mount();
    fireEvent.click(row("第三场"), { shiftKey: true });
    expect(selected()).toHaveLength(3);
  });

  it("无修饰键的单击照旧是「打开这一条」", () => {
    mount();
    fireEvent.click(row("第二场"));
    expect(current()).toEqual([expect.stringContaining("第二场")]);
    expect(selected()).toEqual([expect.stringContaining("第二场")]);
  });

  it("右键落在选中集里就整批交出去，落在集外则收敛成那一条", () => {
    mount();
    fireEvent.click(row("第二场"), { metaKey: true });
    fireEvent.contextMenu(row("第一场"));
    expect(lastMenuTargets()).toEqual(["Chats/一.md", "Chats/二.md"]);
    fireEvent.contextMenu(row("第三场"));
    expect(lastMenuTargets()).toEqual(["Chats/三.md"]);
  });

  it("右键**不**顺带打开——那会把正在聊的这一场连同没发出去的那句一起换掉", () => {
    mount();
    fireEvent.contextMenu(row("第三场"));
    expect(current()).toEqual([expect.stringContaining("第一场")]);
  });

  it("行内删除按钮交的是一批而不是一条（与右键菜单同一条通道）", () => {
    mount();
    fireEvent.click(screen.getAllByRole("button", { name: "删除会话" })[0]);
    expect(deleted).toEqual([["Chats/一.md"]]);
  });
});
