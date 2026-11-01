// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));

import { ContextMenuHost } from "../components/ContextMenuHost";
import { withI18n } from "../i18n/i18n-test-fixture";
import { ChatHistoryList } from "../features/chat/ChatHistoryList";
import { FilteredNoteList } from "../features/workspace/FilteredNoteList";
import { ListLoadMore, type ListLoadState } from "../features/workspace/ListLoadMore";
import { useNoteSelection } from "../features/workspace/use-note-selection";

function Harness({ kind, state }: { kind: "notes" | "chat"; state: ListLoadState }) {
  const selection = useNoteSelection({ current: null, visible: [] });
  const footer = (
    <ListLoadMore {...state} onLoadMore={() => {}} onRetry={() => {}} />
  );
  return kind === "notes" ? (
    <FilteredNoteList
      items={[]}
      loadState={state}
      selectedPath={null}
      onSelect={() => {}}
      noteType="recording"
      selection={selection}
      footer={footer}
      emptyState={<div data-testid="empty" />}
    />
  ) : (
    <ChatHistoryList
      entries={[]}
      total={0}
      loadState={state}
      activePath={null}
      canOpenInNotes={false}
      onNewChat={() => {}}
      onOpen={() => {}}
      onDelete={() => {}}
      onItemContextMenu={() => {}}
      selection={selection}
      footer={footer}
    />
  );
}

afterEach(cleanup);

describe.each(["notes", "chat"] as const)("%s 首次取数的真实状态", (kind) => {
  it.each([
    { loading: false, failed: false, exhausted: false },
    { loading: true, failed: false, exhausted: false },
    { loading: false, failed: true, exhausted: false },
    { loading: false, failed: false, exhausted: true },
  ])("$loading/$failed/$exhausted 只有成功取完后才能说为空", (state) => {
    const view = render(withI18n(
      <ContextMenuHost><Harness kind={kind} state={state} /></ContextMenuHost>,
    ));
    const empty = view.container.querySelector('.luna-empty-state, [data-testid="empty"]');
    expect(empty !== null).toBe(state.exhausted);
    expect(view.container.querySelector(".luna-count-badge") !== null).toBe(state.exhausted);
    if (state.failed) expect(view.getByRole("alert")).toBeDefined();
    if (state.loading) {
      expect(view.container.querySelector('[aria-busy="true"]')).not.toBeNull();
      expect(view.getByRole("button", { name: "正在加载…" }).hasAttribute("disabled")).toBe(true);
    }
  });
});
