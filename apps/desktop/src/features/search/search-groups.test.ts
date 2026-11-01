// 搜索命中三段分桶的守护（ADR-024 来源轴 × ADR-086 成熟度轴 / PRD-06-052）。

import { describe, expect, it } from "vitest";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import type { SearchNoteDto } from "../../ipc/bindings";
import { groupSearchHits } from "./search-groups";

function hit(path: string, flags: { material?: boolean; draft?: boolean } = {}): SearchNoteDto {
  return {
    path,
    title: path,
    modified_ms: 0,
    note_type: flags.material ? "transcript" : "note",
    source_material: flags.material ?? false,
    draft: flags.draft ?? false,
    match_count: 1,
    lines: [],
    lines_truncated: false,
  };
}

describe("groupSearchHits", () => {
  it("三段各就各位：知识 / 记录 / 资料", () => {
    const groups = groupSearchHits([
      hit("Notes/定稿.md"),
      hit("Notes/草稿.md", { draft: true }),
      hit("Transcripts/会议.md", { material: true }),
    ]);
    expect(groups.knowledge.map((n) => n.path)).toEqual(["Notes/定稿.md"]);
    expect(groups.draft.map((n) => n.path)).toEqual(["Notes/草稿.md"]);
    expect(groups.material.map((n) => n.path)).toEqual(["Transcripts/会议.md"]);
  });

  it("不降权：桶内**保持 Rust 给的次序**，前端一个位置都不动", () => {
    // 分组只回答「这是草稿」，不惩罚排序（PRD-06-052）。这条一旦破了，
    // 界面上只表现为"记录好像总排在后面"，没有任何东西会因此变红。
    const notes = [
      hit("a.md", { draft: true }),
      hit("b.md", { draft: true }),
      hit("c.md", { draft: true }),
    ];
    expect(groupSearchHits(notes).draft.map((n) => n.path)).toEqual(["a.md", "b.md", "c.md"]);
  });

  it("来源轴优先：资料笔记即便带着 draft 也归资料（三态互斥，与 Rust 同口径）", () => {
    const groups = groupSearchHits([hit("Chats/x.md", { material: true, draft: true })]);
    expect(groups.material).toHaveLength(1);
    expect(groups.draft).toHaveLength(0);
  });

  it("不重不漏：每条命中恰好落一个桶", () => {
    const notes = [
      hit("1.md"),
      hit("2.md", { draft: true }),
      hit("3.md", { material: true }),
      hit("4.md", { material: true, draft: true }),
    ];
    const groups = groupSearchHits(notes);
    expect(groups.knowledge.length + groups.draft.length + groups.material.length).toBe(
      notes.length,
    );
  });

  it("空结果给三个空桶（组头据此不渲染，而不是渲染一个 0）", () => {
    expect(groupSearchHits([])).toEqual({ knowledge: [], draft: [], material: [] });
  });

  it("两个组头文案在中英双语都在场", () => {
    for (const bundle of [zhWorkspace, enWorkspace]) {
      expect(bundle.search.draftGroup.length).toBeGreaterThan(0);
      expect(bundle.search.materialGroup.length).toBeGreaterThan(0);
    }
  });
});
