// @vitest-environment happy-dom
/**
 * 速记行的两层结构（PRD-06-058 A/G）。
 *
 * 真机 2026-09-09：加上「待办 0/2」徽标之后，第一行只剩徽标与时龄，标题一个字都不剩。
 * 根因是行内的元信息全是 `flex-shrink: 0`，而标题能收缩到 0——于是每加一枚徽标，标题就
 * 被挤掉一截，加到第二枚就整个没了。
 *
 * 这不是调一次 `min-width` 能了结的事：只要内容元信息还和标题抢同一行，下一个徽标就会
 * 再犯一次。故收成结构约束——标题独占第一行，内容元信息（预览 / 待办 / 时长）住第二行，
 * 由这条用例钉住。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { ContextMenuHost } from "../../components/ContextMenuHost";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { DraftInbox } from "./DraftInbox";
import type { DraftNoteDto } from "../../ipc/bindings";
import { ListLoadMore, type ListLoadState } from "../workspace/ListLoadMore";

function note(overrides: Partial<DraftNoteDto> = {}): DraftNoteDto {
  return {
    path: "QuickNotes/买菜.md",
    title: "买菜清单",
    preview: "买牛奶",
    modified_ms: Date.now(),
    quick_note: true,
    refined: false,
    has_media: false,
    duration_ms: 0,
    stale: false,
    can_merge: true,
    pinned_to_desktop: false,
    todo_open: 1,
    todo_total: 2,
    ...overrides,
  } as DraftNoteDto;
}

function mount(rows: DraftNoteDto[], loadState: ListLoadState = { loading: false, failed: false, exhausted: true }) {
  // 行支持右键菜单，故要挂宿主（与主窗口同层，`main.tsx`）。
  return render(
    withI18n(
      <ContextMenuHost>
      <DraftInbox
        notes={rows}
        loadState={loadState}
        footer={
          <ListLoadMore
            {...loadState}
            onLoadMore={() => {}}
            onRetry={() => {}}
          />
        }
        total={rows.length}
        filter="all"
        onFilterChange={() => {}}
        query=""
        onQueryChange={() => {}}
        folders={{ rootDir: undefined, dirs: [] } as never}
        selection={
          {
            ordered: [],
            multiple: false,
            contains: () => false,
            click: () => true,
            contextClick: () => {},
            targetsFor: (path: string) => [path],
            containerProps: { tabIndex: 0, onKeyDown: () => {} },
            rowProps: () => ({ onMouseDown: () => {} }),
          } as never
        }
        onVisibleRows={() => {}}
        selectedPath={null}
        onSelect={() => {}}
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
      </ContextMenuHost>,
    ),
  );
}

afterEach(cleanup);

describe("速记行的两层结构", () => {
  it.each([
    { loading: false, failed: false, exhausted: false },
    { loading: true, failed: false, exhausted: false },
    { loading: false, failed: true, exhausted: false },
  ])("首屏取数状态 $loading/$failed 不宣称为空或零条", (loadState) => {
    const view = mount([], loadState);
    expect(view.container.querySelector(".luna-empty-state")).toBeNull();
    expect(view.container.querySelector(".luna-count-badge")).toBeNull();
    expect(view.container.querySelector(".luna-list-more")).not.toBeNull();
    if (loadState.failed) expect(view.getByRole("alert")).toBeDefined();
    expect(view.container.querySelector("section")?.getAttribute("aria-busy"))
      .toBe(String(loadState.loading));
  });

  it("取数成功后零条显示空态，下一页失败保留已有条目", () => {
    const empty = mount([]);
    expect(empty.container.querySelector(".luna-empty-state")).not.toBeNull();
    expect(empty.container.querySelector(".luna-count-badge")?.textContent).toBe("0");
    empty.unmount();
    const failed = mount([note()], { loading: false, failed: true, exhausted: false });
    expect(failed.container.querySelector(".luna-inbox__row-title")?.textContent)
      .toBe("买菜清单");
    expect(failed.getByRole("alert")).toBeDefined();
    expect(failed.container.querySelector(".luna-empty-state")).toBeNull();
  });

  it("标题独占第一行——内容元信息一律不与它同层", () => {
    const view = mount([note({ has_media: true, duration_ms: 8_000 })]);
    const body = view.container.querySelector(".luna-inbox__row-body")!;
    const title = body.querySelector(".luna-inbox__row-title")!;
    const meta = body.querySelector(".luna-inbox__row-meta")!;

    // 标题与元信息行是兄弟，且元信息在标题之后（读屏与视觉都是先身份、后细节）。
    expect(title.parentElement).toBe(body);
    expect(meta.parentElement).toBe(body);
    expect(title.compareDocumentPosition(meta) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    // 三枚内容元信息全在第二行里——搬回标题那一层，标题就会重新被挤掉。
    for (const cls of ["__row-preview", "__row-todos", "__row-duration"]) {
      expect(meta.querySelector(`.luna-inbox${cls}`), cls).not.toBeNull();
      expect(title.querySelector(`.luna-inbox${cls}`), cls).toBeNull();
    }
  });

  it("时龄与「AI 已整理」角标也住第二行，不与标题主体同层抢宽度", () => {
    // 真机 2026-10-06：它们在主体外且不收缩，英文时龄 + 150% 字号把主体挤成零宽，标题消失。
    const view = mount([note({ preview: "", todo_total: 0, todo_open: 0, refined: true })]);
    const meta = view.container.querySelector(".luna-inbox__row-meta")!;
    expect(meta.querySelector(".luna-inbox__row-age")).not.toBeNull();
    expect(meta.querySelector(".luna-inbox__row-refined")).not.toBeNull();
    const open = view.container.querySelector(".luna-inbox__row-open")!;
    for (const cls of ["__row-age", "__row-refined"]) {
      expect(open.querySelector(`:scope > .luna-inbox${cls}`), cls).toBeNull();
    }
  });

  it("待办全部勾完时换一种说法，而不是再数一遍", () => {
    const view = mount([note({ todo_open: 0, todo_total: 3 })]);
    const todos = view.container.querySelector(".luna-inbox__row-todos")!;
    expect(todos.textContent).not.toContain("3/3");
    expect(todos.getAttribute("data-done")).not.toBeNull();
  });
});
