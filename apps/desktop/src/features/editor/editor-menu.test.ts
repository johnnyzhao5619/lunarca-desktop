import { describe, expect, it } from "vitest";
import { editorContextMenuItems } from "./editor-actions";
import type { FormatState } from "./format-commands";
import type { ContextMenuAction, ContextMenuItem } from "../../lib/context-menu";

const PLAIN: FormatState = {
  strong: false,
  em: false,
  del: false,
  code: false,
  heading: 0,
  bullet: false,
  ordered: false,
  task: false,
  quote: false,
};

function build(
  formatState: FormatState,
  hasSelection: boolean,
  glossaryTerm: string | null = null,
  markableAsDraft = false,
  templatesAvailable = true,
) {
  return editorContextMenuItems({
    templatesAvailable,
    aiShortcut: "cmd_or_ctrl+j",
    t: (key) => key,
    formatState,
    hasSelection,
    onAction: () => {},
    onAiActions: () => {},
    onReadSelection: () => {},
    glossaryTerm,
    onAddToGlossary: () => {},
    onTidyListLevels: () => {},
    onMarkAsDraft: markableAsDraft ? () => {} : null,
  });
}

/** 取子菜单里的可执行项（分隔线不参与断言）。 */
function submenu(items: ContextMenuItem[], id: string): ContextMenuAction[] {
  // 只认子菜单：分隔线可以与子菜单同名（插入组前那条分隔线也叫 insert），先命中它会让断言空跑。
  const found = items.find((item) => item.id === id && item.kind === "submenu");
  if (found?.kind !== "submenu") {
    throw new Error(`菜单里没有子菜单 ${id}`);
  }
  const children = found.items;
  return children.filter((item): item is ContextMenuAction => item.kind === "action");
}

/** 按 id 取可执行项（分隔线没有禁用态，断言前先收窄类型）。 */
function actionById(items: ContextMenuItem[], id: string): ContextMenuAction {
  const found = items.find((item) => item.id === id);
  if (found?.kind !== "action") {
    throw new Error(`菜单里没有可执行项 ${id}`);
  }
  return found;
}

describe("editorContextMenuItems", () => {
  it("光标处的格式态以勾选呈现（工具栏与右键同一判定来源）", () => {
    const items = build({ ...PLAIN, strong: true, heading: 2 }, false);
    const format = submenu(items, "textFormat");
    expect(format.find((item) => item.id === "bold")?.checked).toBe(true);
    expect(format.find((item) => item.id === "italic")?.checked).toBe(false);
    const paragraph = submenu(items, "paragraph");
    expect(paragraph.find((item) => item.id === "h2")?.checked).toBe(true);
    expect(paragraph.find((item) => item.id === "text")?.checked).toBe(false);
  });

  it("无选区时剪切/复制/朗读所选置灰，粘贴与全选照常", () => {
    const items = build(PLAIN, false);
    expect(actionById(items, "cut").disabled).toBe(true);
    expect(actionById(items, "copy").disabled).toBe(true);
    expect(actionById(items, "readSelection").disabled).toBe(true);
    expect(actionById(items, "paste").disabled).toBeUndefined();
    expect(actionById(items, "selectAll").disabled).toBeUndefined();
  });

  it("只有真的绑了键的条目才显示快捷键提示（绝不显示按不出效果的组合）", () => {
    const items = build(PLAIN, true);
    const format = submenu(items, "textFormat");
    expect(format.find((item) => item.id === "bold")?.shortcut).toBeTruthy();
    expect(format.find((item) => item.id === "strike")?.shortcut).toBeUndefined();
    const insert = submenu(items, "insert");
    expect(insert.find((item) => item.id === "codeBlock")?.shortcut).toBeUndefined();
  });
});

describe("整理列表层级条目（PRD-06-056）", () => {
  it("无论有无选区都在场（对象是整篇正文，不依赖选区）", () => {
    expect(build(PLAIN, false).some((item) => item.id === "tidyListLevels")).toBe(true);
    const item = actionById(build(PLAIN, true), "tidyListLevels");
    expect(item.disabled).toBeUndefined();
    expect(item.shortcut).toBeUndefined();
  });
});

describe("就地加词条目（PRD-07-061）", () => {
  it("选区像一条词条时出现", () => {
    const items = build(PLAIN, true, "Kubernetes");
    expect(items.some((item) => item.id === "addToGlossary")).toBe(true);
  });

  it("选区不像词条时**不渲染**而不是渲染一个禁用项", () => {
    // 永远点不动的菜单项只是噪声——用户此刻并没有在找这个功能。
    expect(build(PLAIN, true, null).some((item) => item.id === "addToGlossary")).toBe(false);
    expect(build(PLAIN, false).some((item) => item.id === "addToGlossary")).toBe(false);
  });
});

describe("「标记为记录」（PRD-06-058 D）", () => {
  it("不能标记时该项**不渲染**，而不是渲染成点不动的禁用项", () => {
    // 判据由 Rust 下发（记录态自己 / 资料 / 主题页 / 知识地图都不给）。一个永远
    // 点不动的菜单项只是噪声——同「就地加词」的既有裁决。
    expect(build(PLAIN, false).some((item) => item.id === "markAsDraft")).toBe(false);
    expect(build(PLAIN, false, null, true).some((item) => item.id === "markAsDraft")).toBe(true);
  });

  it("它与「整理列表层级」同组——两者都是对整篇的动作", () => {
    const items = build(PLAIN, false, null, true);
    const ids = items.map((item) => item.id);
    expect(ids.indexOf("markAsDraft")).toBe(ids.indexOf("tidyListLevels") + 1);
  });
});

describe("editor context menu · AI 入口", () => {
  it("lists AI actions first, enabled with or without a selection", () => {
    const withSelection = build(PLAIN, true);
    expect(withSelection[0]).toMatchObject({ id: "aiActions", label: "editor:menu.aiActions" });
    expect(actionById(withSelection, "aiActions").disabled).toBeFalsy();
    expect(actionById(build(PLAIN, false), "aiActions").disabled).toBeFalsy();
  });
});

describe("editor context menu · 插入模板（V6）", () => {
  it("插入子菜单里有「插入模板…」，副库窗口里不给", () => {
    const ids = (templatesAvailable: boolean) =>
      submenu(build(PLAIN, false, null, false, templatesAvailable), "insert").map((item) => item.id);
    expect(ids(true)).toContain("template");
    expect(ids(false)).not.toContain("template");
  });
});

describe("拼写建议（W6a）", () => {
  function withSpelling(guesses: string[]) {
    const replaced: string[] = [];
    let learned = false;
    const items = editorContextMenuItems({
      templatesAvailable: true,
      aiShortcut: "",
      t: (key) => key,
      formatState: PLAIN,
      hasSelection: false,
      onAction: () => {},
      onAiActions: () => {},
      onReadSelection: () => {},
      glossaryTerm: null,
      onAddToGlossary: () => {},
      onTidyListLevels: () => {},
      onMarkAsDraft: null,
      spelling: { guesses, onReplace: (guess) => replaced.push(guess), onLearn: () => (learned = true) },
    });
    return { items, replaced, learned: () => learned };
  }

  it("候选排在最上面，最多五个，点了即替换；其后是「学习拼写」与分隔线", () => {
    const { items, replaced, learned } = withSpelling(["world", "would", "word", "wold", "worlds", "words"]);
    const ids = items.map((item) => item.id);
    expect(ids.slice(0, 7)).toEqual([
      "spelling-0",
      "spelling-1",
      "spelling-2",
      "spelling-3",
      "spelling-4",
      "spelling-learn",
      "spelling",
    ]);
    const first = items[0] as ContextMenuAction;
    expect(first.label).toBe("world");
    first.onSelect();
    expect(replaced).toEqual(["world"]);
    (items[5] as ContextMenuAction).onSelect();
    expect(learned()).toBe(true);
  });

  it("没有候选时给一条点不动的提示，「学习拼写」照样在", () => {
    const { items } = withSpelling([]);
    const none = items[0] as ContextMenuAction;
    expect(none.id).toBe("spelling-none");
    expect(none.disabled).toBe(true);
    expect(none.label).toBe("editor:menu.noSpellingGuesses");
    expect(items[1].id).toBe("spelling-learn");
  });

  it("没拼错就整组不出现", () => {
    expect(build(PLAIN, false).some((item) => item.id.startsWith("spelling"))).toBe(false);
  });
});

