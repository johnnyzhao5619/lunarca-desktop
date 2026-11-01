// @vitest-environment happy-dom
/**
 * 知识网络的渲染异常兜底（K7）：画布视图抛错时退回同一份数据的关系列表，节点仍可打开。
 */

import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { GraphView } from "./GraphView";
import type { GraphController } from "./use-graph-controller";

afterEach(cleanup);

describe("GraphView 渲染异常兜底", () => {
  it("画布视图抛错时显示关系列表与重试，节点照样能打开", () => {
    const filtered = {
      nodes: [
        { id: "Notes/a.md", label: "a", kind: "note", note_type: "note", draft: false, degree: 0 },
      ],
      edges: [],
    };
    const controller = {
      get data(): never {
        throw new Error("boom");
      },
      filtered,
    } as unknown as GraphController;
    const onOpenNote = vi.fn();
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      withI18n(
        createElement(GraphView, {
          controller,
          focus: null,
          onOpenNote,
          onOpenTag: vi.fn(),
          onStatus: vi.fn(),
        }),
      ),
    );
    quiet.mockRestore();
    expect(screen.getByText(t("workspace:graph.list.retry"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:graph.list.open") }));
    expect(onOpenNote).toHaveBeenCalledWith("Notes/a.md");
  });
});
