// @vitest-environment happy-dom
/**
 * 知识网络关系列表（K7）：与画布同一份 DTO，键盘可达，每条关系都能打开。
 */

import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { GraphRelationList } from "./GraphRelationList";

afterEach(cleanup);

const NODES = [
  { id: "Notes/a.md", label: "a", kind: "note", note_type: "note", draft: false, degree: 2 },
  { id: "Notes/b.md", label: "b", kind: "note", note_type: "note", draft: false, degree: 1 },
  { id: "tag:项目", label: "#项目", kind: "tag", note_type: null, draft: false, degree: 1 },
];
const EDGES = [
  { source: "Notes/a.md", target: "Notes/b.md", kind: "link", strength: 100 },
  { source: "Notes/a.md", target: "tag:项目", kind: "tag", strength: 100 },
];

describe("GraphRelationList", () => {
  it("按度数列出节点，展开后按种类列出关系，点关系目标即打开它", () => {
    const onOpen = vi.fn();
    render(withI18n(createElement(GraphRelationList, { nodes: NODES, edges: EDGES, onOpen })));
    const labels = screen.getAllByText(/^(a|b|#项目)$/).map((el) => el.textContent);
    expect(labels.slice(0, 3)).toEqual(["a", "b", "#项目"]);
    expect(screen.getByText(t("workspace:graph.list.relations", { count: 2 }))).toBeTruthy();
    expect(screen.getAllByText(t("workspace:graph.list.kind.links_to")).length).toBe(1);
    fireEvent.click(screen.getAllByRole("button", { name: "#项目" })[0]);
    expect(onOpen).toHaveBeenCalledWith({ id: "tag:项目", kind: "tag" });
    fireEvent.click(screen.getAllByRole("button", { name: t("workspace:graph.list.open") })[0]);
    expect(onOpen).toHaveBeenCalledWith(NODES[0]);
  });
});
