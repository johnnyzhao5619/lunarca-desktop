// @vitest-environment happy-dom
/**
 * 空库的知识库工作台（真机 2026-10-06）：一篇知识笔记、一份来源都没有时，主区不能只剩推荐卡
 * 一句「暂时不用整理。」——那像整理完了，其实是还没开始。换成引导空态并给写第一篇的入口。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { isKnowledgeEmpty } from "./KnowledgeStatus";
import { KnowledgeWorkbench } from "./KnowledgeWorkbench";

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("checkupReport", { status: "ok", data: { findings: [] } });
});

const overview = (knowledge_notes: number, materials: number) => ({
  knowledge_notes,
  materials,
  unlinked_notes: 0,
  active_tags: [],
  revisit: null,
});

describe("知识库工作台的空库引导", () => {
  it("空库：说清知识库从哪来，按钮新建笔记；不再出现「暂时不用整理」", async () => {
    setCommandReply("knowledgeOverview", { status: "ok", data: overview(0, 0) });
    const onCreateNote = vi.fn();
    render(withI18n(createElement(KnowledgeWorkbench, { onCreateNote })));
    expect(await screen.findByText(t("workspace:knowledge.landing.title"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:graph.nextAction.empty"))).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:create.newNote") }));
    expect(onCreateNote).toHaveBeenCalledTimes(1);
  });

  it("有笔记或有来源就不算空", () => {
    expect(isKnowledgeEmpty(overview(0, 0))).toBe(true);
    expect(isKnowledgeEmpty(overview(1, 0))).toBe(false);
    expect(isKnowledgeEmpty(overview(0, 2))).toBe(false);
  });
});
