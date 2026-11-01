// @vitest-environment happy-dom
/**
 * 快速切换器（PRD-07-040，笔记库对标轮 W1）：最近打开、拼音、别名、⇧Enter 新建、⌘Enter 分屏。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { QuickSwitcher } from "./QuickSwitcher";
import { matchSwitcherItem, type QuickSwitcherItem } from "./quick-switcher-match";

const ok = (data: unknown) => ({ status: "ok", data });
const flush = () => act(async () => {});

function entry(path: string, extra: Partial<Record<string, unknown>> = {}) {
  const title = (path.split("/").pop() ?? path).replace(/\.md$/, "");
  return { path, title, aliases: [], pinyin: [], is_source_material: false, ...extra };
}

function item(path: string, extra: Partial<QuickSwitcherItem> = {}): QuickSwitcherItem {
  const title = (path.split("/").pop() ?? path).replace(/\.md$/, "");
  return { path, title, searchText: `${title} ${path}`, pinyin: [], isSourceMaterial: false, ...extra };
}

const noRanks = { recent: new Map<string, number>(), content: new Map<string, number>() };

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("matchSwitcherItem", () => {
  it("空查询只列最近打开、新在前；没有最近打开时列全部", () => {
    const recent = new Map([
      ["b.md", 0],
      ["a.md", 1],
    ]);
    const ranks = { recent, content: new Map<string, number>() };
    expect(matchSwitcherItem(item("c.md"), "", ranks)).toBeNull();
    const b = matchSwitcherItem(item("b.md"), "", ranks)!;
    const a = matchSwitcherItem(item("a.md"), "", ranks)!;
    expect(b.score).toBeGreaterThan(a.score);
    expect(matchSwitcherItem(item("c.md"), "", noRanks)).not.toBeNull();
  });

  it("拼音命中排在模糊命中之后、正文命中之前", () => {
    const meeting = item("会议.md", { pinyin: ["huiyi", "hy"] });
    const english = item("hybrid.md");
    const content = { recent: new Map<string, number>(), content: new Map([["其它.md", 0]]) };
    const viaPinyin = matchSwitcherItem(meeting, "hy", content)!;
    const viaTitle = matchSwitcherItem(english, "hy", content)!;
    const viaContent = matchSwitcherItem(item("其它.md"), "hy", content)!;
    expect(viaTitle.score).toBeGreaterThan(viaPinyin.score);
    expect(viaPinyin.score).toBeGreaterThan(viaContent.score);
    expect(matchSwitcherItem(meeting, "会", noRanks)).not.toBeNull();
    expect(matchSwitcherItem(meeting, "zz", noRanks)).toBeNull();
  });
});

describe("QuickSwitcher", () => {
  function open(onOpenPath = vi.fn(), onOpenInSplit = vi.fn()) {
    const view = render(
      withI18n(
        <QuickSwitcher
          open
          onClose={() => {}}
          onOpenPath={onOpenPath}
          onOpenInSplit={onOpenInSplit}
        />,
      ),
    );
    return { view, onOpenPath, onOpenInSplit };
  }

  it("空查询列最近打开，别名与拼音能找到笔记", async () => {
    setCommandReply(
      "quickSwitcher",
      ok({
        entries: [
          entry("项目/会议纪要.md", { aliases: ["周会"], pinyin: ["huiyijiyao", "hyjy", "zhouhui", "zh"] }),
          entry("Weekly.md"),
        ],
        recent: ["Weekly.md"],
      }),
    );
    const { view } = open();
    await flush();
    const rows = () => [...view.container.querySelectorAll("[role=option] strong")].map((el) => el.textContent);
    expect(rows()).toEqual(["Weekly"]);
    const input = view.getByRole("searchbox");
    fireEvent.change(input, { target: { value: "周会" } });
    expect(rows()).toEqual(["会议纪要"]);
    fireEvent.change(input, { target: { value: "hyjy" } });
    expect(rows()).toEqual(["会议纪要"]);
  });

  it("⇧Enter 按查询词新建并打开，⌘Enter 在分屏打开", async () => {
    setCommandReply("quickSwitcher", ok({ entries: [entry("Weekly.md")], recent: [] }));
    setCommandReply("createNoteNamed", ok({ path: "Notes/新想法.md", existed: false }));
    const { view, onOpenPath, onOpenInSplit } = open();
    await flush();
    const input = view.getByRole("searchbox");
    fireEvent.change(input, { target: { value: "Weekly" } });
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    expect(onOpenInSplit).toHaveBeenCalledWith("Weekly.md");
    fireEvent.change(input, { target: { value: "新想法" } });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    await flush();
    expect(commandMock("createNoteNamed")).toHaveBeenCalledWith("新想法");
    expect(onOpenPath).toHaveBeenCalledWith("Notes/新想法.md");
  });
});
