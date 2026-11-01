// @vitest-environment happy-dom
/**
 * 主题级知识网络（PRD-16-062 / ADR-136 ④）：默认主题网络、库里还没有主题页时落到笔记网络；
 * 主题节点聚焦（单击）即预览、回车（双击）才打开；预览卡只渲染 Rust 给的范围与结论。
 */

import { createElement } from "react";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { PromptHost } from "../../components/PromptHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { TopicPreviewCard } from "../knowledge/TopicPreviewCard";
import { GraphView } from "./GraphView";
import { relationsOf } from "./graph-sim";
import { useGraphController, type GraphController } from "./use-graph-controller";

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("listSettings", { status: "ok", data: [] });
  setCommandReply("graphRescanStatus", { status: "ok", data: null });
});

const topic = (id: string, label: string, degree = 1) => ({
  id,
  label,
  kind: "topic",
  note_type: null,
  draft: false,
  degree,
});
const NETWORK = {
  nodes: [topic("Knowledge/甲.md", "项目甲"), topic("Knowledge/乙.md", "项目乙")],
  edges: [{ source: "Knowledge/甲.md", target: "Knowledge/乙.md", kind: "source", strength: 100 }],
};

function routes() {
  const seen: string[] = [];
  const onNavigate = (event: Event) => {
    seen.push((event as CustomEvent<string>).detail);
    event.preventDefault();
  };
  window.addEventListener(NAVIGATE_EVENT, onNavigate);
  return { seen, dispose: () => window.removeEventListener(NAVIGATE_EVENT, onNavigate) };
}

describe("useGraphController 的网络粒度", () => {
  it("默认取主题网络", async () => {
    setCommandReply("topicNetwork", { status: "ok", data: NETWORK });
    const { result } = renderHook(() => useGraphController(true));
    await vi.waitFor(() => expect(result.current.data).toEqual(NETWORK));
    expect(result.current.mode).toBe("topics");
    expect(commandMock("vaultGraph")).not.toHaveBeenCalled();
  });

  it("库里还没有主题页时落到笔记网络；用户亲手选了主题就不再替他换", async () => {
    setCommandReply("topicNetwork", { status: "ok", data: { nodes: [], edges: [] } });
    setCommandReply("vaultGraph", { status: "ok", data: { nodes: [], edges: [] } });
    const { result } = renderHook(() => useGraphController(true));
    await vi.waitFor(() => expect(result.current.mode).toBe("notes"));
    await vi.waitFor(() => expect(commandMock("vaultGraph")).toHaveBeenCalled());
    act(() => result.current.setMode("topics"));
    await vi.waitFor(() => expect(result.current.data).toEqual({ nodes: [], edges: [] }));
    expect(result.current.mode).toBe("topics");
  });
});

describe("主题节点：预览与打开分开", () => {
  function mount(preview: string | null, setPreview = vi.fn()) {
    const controller = {
      mode: "topics",
      preview,
      setPreview,
      data: NETWORK,
      error: null,
      filtered: NETWORK,
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
              createElement(GraphView, { controller, focus: null, onOpenNote: vi.fn(), onOpenTag: vi.fn() }),
            ),
          ),
        ),
      ),
    );
    return setPreview;
  }

  it("键盘聚焦主题节点即预览，回车才打开主题阅读视图", () => {
    const nav = routes();
    const setPreview = mount(null);
    const node = screen.getByText(
      t("workspace:graph.accessibleTopicNode", { label: "项目甲", count: 1 }),
    );
    fireEvent.focus(node);
    expect(setPreview).toHaveBeenCalledWith("Knowledge/甲.md");
    expect(nav.seen).toEqual([]);
    fireEvent.click(node);
    expect(nav.seen).toEqual(["topics://Knowledge/甲.md"]);
    nav.dispose();
  });

  it("预览中的那一页仍在图上时给预览卡", async () => {
    setCommandReply("topicPreview", {
      status: "ok",
      data: {
        path: "Knowledge/甲.md",
        title: "项目甲",
        summary: "",
        scope: null,
        claim_count: 0,
        conflict_count: 0,
        claims: [],
      },
    });
    mount("Knowledge/甲.md");
    expect(await screen.findByRole("complementary", { name: t("workspace:topicPreview.label") })).toBeTruthy();
    expect(commandMock("topicPreview")).toHaveBeenCalledWith("Knowledge/甲.md");
  });
});

describe("TopicPreviewCard", () => {
  it("摆范围、计数与站得住的结论；打开交回路径", async () => {
    setCommandReply("topicPreview", {
      status: "ok",
      data: {
        path: "Knowledge/甲.md",
        title: "项目甲",
        summary: "围绕后端迁移的进展。",
        scope: { title: "项目甲", description: null, includes: ["项目/甲"], excludes: [] },
        claim_count: 4,
        conflict_count: 1,
        claims: [{ text: "后端改用 PostgreSQL", citations: [{ citation_id: "c1", source_title: "周会" }] }],
      },
    });
    const onOpen = vi.fn();
    render(withI18n(createElement(TopicPreviewCard, { path: "Knowledge/甲.md", onOpen, onClose: vi.fn() })));
    expect(await screen.findByText("后端改用 PostgreSQL")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicPreview.scope", { tags: "#项目/甲" }))).toBeTruthy();
    expect(
      screen.getByText(t("workspace:topicPreview.countsWithConflicts", { count: 4, conflicts: 1 })),
    ).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:topicPreview.open")));
    expect(onOpen).toHaveBeenCalledWith("Knowledge/甲.md");
  });

  it("旧格式页没有范围与结论时如实说明", async () => {
    setCommandReply("topicPreview", {
      status: "ok",
      data: {
        path: "Knowledge/旧.md",
        title: "旧",
        summary: "一段旧式正文。",
        scope: null,
        claim_count: 0,
        conflict_count: 0,
        claims: [],
      },
    });
    render(withI18n(createElement(TopicPreviewCard, { path: "Knowledge/旧.md", onOpen: vi.fn(), onClose: vi.fn() })));
    expect(await screen.findByText(t("workspace:topicPreview.legacy"))).toBeTruthy();
    expect(screen.getByText("一段旧式正文。")).toBeTruthy();
  });
});

describe("relationsOf 的共享来源", () => {
  it("共享来源是对称关系：两端各记一次", () => {
    expect(relationsOf("Knowledge/甲.md", NETWORK.nodes, NETWORK.edges)).toEqual([
      { id: "Knowledge/乙.md", label: "项目乙", kind: "shares_source" },
    ]);
    expect(relationsOf("Knowledge/乙.md", NETWORK.nodes, NETWORK.edges)).toEqual([
      { id: "Knowledge/甲.md", label: "项目甲", kind: "shares_source" },
    ]);
  });
});
