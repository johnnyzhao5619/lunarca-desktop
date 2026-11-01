// @vitest-environment happy-dom
/**
 * 知识网络的视角记忆（K7「打开后返回图谱原位置」）：离开时记下位置与视角，回来的是同一张图
 * 才沿用；换了库或库大变时照首次进入处理。
 */

import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { PromptHost } from "../../components/PromptHost";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { GraphView } from "./GraphView";
import {
  memoryFits,
  recallGraphView,
  rememberGraphView,
  type GraphViewMemory,
} from "./graph-view-memory";
import type { SimNode } from "./graph-sim";
import type { GraphController } from "./use-graph-controller";

afterEach(cleanup);

const node = (id: string, x: number): SimNode => ({
  id,
  label: id,
  kind: "note",
  noteType: "note",
  degree: 0,
  x,
  y: 0,
  vx: 0,
  vy: 0,
  pinned: false,
});

const remembered = (ids: string[]): GraphViewMemory => ({
  positions: new Map(ids.map((id, index) => [id, node(id, index * 10)])),
  transform: { x: 5, y: 6, scale: 2 },
  alpha: 0,
  fitPending: false,
  view: "canvas",
});

describe("memoryFits", () => {
  it("当前节点至少一半记得位置才沿用；没有记忆或没有节点都不沿用", () => {
    expect(memoryFits(remembered(["a", "b"]), ["a", "b", "c"])).toBe(true);
    expect(memoryFits(remembered(["a", "b"]), ["a", "b", "c", "d"])).toBe(true);
    expect(memoryFits(remembered(["a"]), ["a", "b", "c"])).toBe(false);
    expect(memoryFits(remembered(["x", "y"]), ["a", "b"])).toBe(false);
    expect(memoryFits(null, ["a"])).toBe(false);
    expect(memoryFits(remembered(["a"]), [])).toBe(false);
  });
});

describe("GraphView 视角记忆", () => {
  it("卸载时记下每个节点的位置；同一张图重挂时沿用记下的位置与视角，而不是重铺", () => {
    const filtered = {
      nodes: [
        { id: "Notes/a.md", label: "a", kind: "note", note_type: "note", draft: false, degree: 1 },
        { id: "Notes/b.md", label: "b", kind: "note", note_type: "note", draft: false, degree: 1 },
      ],
      edges: [{ source: "Notes/a.md", target: "Notes/b.md", kind: "link", strength: 100 }],
    };
    const controller = {
      data: filtered,
      error: null,
      filtered,
      reload: vi.fn(),
      showTags: true,
      toggleTags: vi.fn(),
      showGhosts: true,
      toggleGhosts: vi.fn(),
      showOrphans: true,
      toggleOrphans: vi.fn(),
      focus: null,
      setFocus: vi.fn(),
      relayoutNonce: 0,
      openRescanDialog: vi.fn(),
    } as unknown as GraphController;
    const mount = () =>
      render(
        withI18n(
          createElement(
            ConfirmHost,
            null,
            createElement(
              PromptHost,
              null,
              createElement(
                ContextMenuHost,
                null,
                createElement(GraphView, {
                  controller,
                  focus: null,
                  onOpenNote: vi.fn(),
                  onOpenTag: vi.fn(),
                }),
              ),
            ),
          ),
        ),
      );

    mount().unmount();
    const memory = recallGraphView();
    expect([...(memory?.positions.keys() ?? [])]).toEqual(["Notes/a.md", "Notes/b.md"]);

    // 记忆里放一组初始螺旋不可能给出的位置：重挂若按初始位置重铺，就对不上。
    rememberGraphView({
      positions: new Map([
        ["Notes/a.md", node("Notes/a.md", 123)],
        ["Notes/b.md", node("Notes/b.md", 456)],
      ]),
      transform: { x: 7, y: 8, scale: 1.5 },
      alpha: 0,
      fitPending: false,
      view: "canvas",
    });
    mount().unmount();
    const after = recallGraphView();
    expect([...(after?.positions.values() ?? [])].map(({ x }) => x)).toEqual([123, 456]);
    expect(after?.transform).toEqual({ x: 7, y: 8, scale: 1.5 });
  });
});
