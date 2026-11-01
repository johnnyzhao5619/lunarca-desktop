import { describe, expect, it } from "vitest";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import type { ProjectCardDto, ProjectStageDto } from "../../ipc/bindings";
import { columnsOf, isDisplacedCard, stageDropId, stageLabelKey, stageOfDropId } from "./creator-board";

/**
 * 全部阶段——**由 Rust 的 `ProjectStageDto` 编译期穷尽**：Rust 加一个阶段、绑定重生成后
 * 这里编译不过，于是阶段名的双语守护会立刻要求补齐它，而不是在看板上渲染出一个裸 key。
 */
const ALL_STAGES: Record<ProjectStageDto, true> = {
  idea: true,
  research: true,
  writing: true,
  recording: true,
  editing: true,
  publishing: true,
  published: true,
};
const STAGES = Object.keys(ALL_STAGES) as ProjectStageDto[];

function card(path: string, stage: ProjectStageDto, column: ProjectStageDto = stage): ProjectCardDto {
  return {
    path,
    title: path,
    stage,
    column,
    folder: "Creator/P",
    modified_ms: 0,
    channels: ["youtube", "bilibili"],
    publish_date: null,
    signals: { script_seconds: null, quotes: 0, publish_done: 0, publish_total: 6, next_action: null },
  };
}

function lookup(bundle: unknown, key: string): unknown {
  return key
    .split(".")
    .reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], bundle);
}

describe("创作空间看板", () => {
  it("按 Rust 给的列序分列，每列保持原顺序，空列照样出现", () => {
    const columns = columnsOf(["idea", "writing", "published"], [
      card("b", "writing"),
      card("a", "idea"),
      card("c", "writing"),
    ]);
    expect(columns.map((column) => column.stage)).toEqual(["idea", "writing", "published"]);
    expect(columns[1].cards.map((c) => c.path)).toEqual(["b", "c"]);
    expect(columns[2].cards).toEqual([]);
  });

  it("按 Rust 给的显示列分组：停在隐藏列上的项目出现在借住的那一列，并被标为借住", () => {
    const parked = card("p", "editing", "writing");
    const columns = columnsOf(["idea", "writing"], [card("a", "idea"), parked]);
    expect(columns[1].cards).toEqual([parked]);
    expect(isDisplacedCard(parked)).toBe(true);
    expect(isDisplacedCard(columns[0].cards[0])).toBe(false);
  });

  it("落点 id 与阶段互转；别的落点或不在列序里的阶段一律取消", () => {
    for (const stage of STAGES) {
      expect(stageOfDropId(stageDropId(stage), STAGES)).toBe(stage);
    }
    expect(stageOfDropId("Notes/a.md", STAGES)).toBeNull();
    expect(stageOfDropId("creator-stage:bogus", STAGES)).toBeNull();
    expect(stageOfDropId(stageDropId("published"), ["idea"])).toBeNull();
  });

  it("每个阶段在中英文里都有名字", () => {
    for (const bundle of [zhWorkspace, enWorkspace]) {
      for (const stage of STAGES) {
        expect(lookup(bundle, stageLabelKey(stage)), stageLabelKey(stage)).toBeTruthy();
      }
    }
  });
});
