// @vitest-environment happy-dom
/**
 * 知识网络第一次读取中（K7）：真机上大一点的库要读几秒，那几秒空画布 + 「0 节点」看起来
 * 就是图谱空了。读取中要说「加载中」，数据回来且确实为空才说空。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { PromptHost } from "../../components/PromptHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { GraphView } from "./GraphView";
import type { GraphController } from "./use-graph-controller";

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  // 画布里的笔记右键菜单挂载时会取一次设置规格（语言选项）。
  setCommandReply("listSettings", { status: "ok", data: [] });
});

function mount(data: { nodes: never[]; edges: never[] } | null) {
  const empty = { nodes: [], edges: [] };
  const controller = {
    data,
    error: null,
    filtered: empty,
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
  return render(
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
}

describe("GraphView 读取中", () => {
  it("数据还没回来时说加载中，不说空", () => {
    mount(null);
    expect(screen.getByText(t("common:status.loading"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:graph.empty"))).toBeNull();
  });

  it("数据回来且确实为空时才说空", () => {
    mount({ nodes: [], edges: [] });
    expect(screen.getByText(t("workspace:graph.empty"))).toBeTruthy();
    expect(screen.queryByText(t("common:status.loading"))).toBeNull();
  });
});
