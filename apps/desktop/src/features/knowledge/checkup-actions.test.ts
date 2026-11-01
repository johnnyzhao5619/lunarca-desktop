import { describe, expect, it } from "vitest";
import type { CheckupFindingDto } from "../../ipc/bindings";
import { fixAction, primaryAction, primaryActionKey } from "./checkup-actions";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";

/**
 * 体检卡动作映射 + 卡片文案的动态 key 守护（PRD-16-030/032）。
 *
 * 种类串是 **Rust 侧的 kebab 键**（`luna_vault::checkup::FindingKind::key`），前端按
 * `checkup.card.<kind>.*` 与 `checkup.action.*` 动态查表——静态扫描 `t("…")` 的正则看不到，
 * 漏写文案时卡片上显示的是裸 key，且 typecheck / lint / 中英 parity 全绿。
 */

/** Rust `FindingKind::ALL` 的镜像；两侧任一改动，这条守护先红。 */
const KINDS = [
  "contradiction",
  "outdated",
  "stale-citation",
  "near-duplicate",
  "broken-link",
  "unreadable-folder",
  "untagged",
  "orphan",
  "empty-note",
  "distill-backlog",
  "settle-backlog",
];

function finding(patch: Partial<CheckupFindingDto>): CheckupFindingDto {
  return {
    id: "orphan:Notes/a.md",
    kind: "orphan",
    paths: ["Notes/a.md"],
    line: null,
    subject: "",
    reason: "",
    count: 0,
    ignorable: true,
    backlog: false,
    fix: null,
    ...patch,
  };
}

type Dict = Record<string, unknown>;

function lookup(bundle: Dict, path: string): string | undefined {
  const value = path.split(".").reduce<unknown>((node, key) => {
    return typeof node === "object" && node !== null ? (node as Dict)[key] : undefined;
  }, bundle);
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

describe("体检卡的动作映射", () => {
  it("指名道姓的发现把用户带到现场（带行号就跳行）", () => {
    expect(primaryAction(finding({ paths: ["Notes/主题.md"], line: 12 }))).toEqual({
      kind: "note",
      path: "Notes/主题.md",
      line: 12,
    });
  });

  it("「还没有标签」通向全库补齐确认卡——它指的是一批笔记，且那张卡还能解除文件夹停用", () => {
    expect(primaryAction(finding({ kind: "untagged", paths: [], count: 12 }))).toEqual({
      kind: "rescan",
    });
    expect(fixAction(finding({ kind: "untagged", paths: [], fix: "rescan" }))).toEqual({
      kind: "rescan",
    });
  });

  it("待整理积压通向批量确认卡——它指的是一堆资料，不是某一篇", () => {
    expect(primaryAction(finding({ kind: "distill-backlog", paths: [], count: 7 }))).toEqual({
      kind: "distill-batch",
    });
  });

  it("待沉淀积压掀开文件树的「隐藏记录」，并带用户去最旧的那一篇（PRD-06-055）", () => {
    expect(
      primaryAction(
        finding({ kind: "settle-backlog", paths: ["Notes/草稿甲.md", "Notes/草稿乙.md"], count: 7 }),
      ),
    ).toEqual({ kind: "show-drafts", path: "Notes/草稿甲.md" });
    // 一篇芯片都没有时照常掀开树——"有 N 篇记录待沉淀"这句话本身仍然成立，
    // 而这一类**没有**「修复」按钮：把草稿判成知识只有用户有资格（PRD-06-053）。
    expect(primaryAction(finding({ kind: "settle-backlog", paths: [], count: 7 }))).toEqual({
      kind: "show-drafts",
      path: null,
    });
    expect(fixAction(finding({ kind: "settle-backlog", paths: [], fix: null }))).toBeNull();
  });

  it("读不了的文件夹在访达里亮出来，而不是当成笔记去打开（V1b）", () => {
    const action = primaryAction(finding({ kind: "unreadable-folder", paths: ["锁住"] }));
    expect(action).toEqual({ kind: "reveal", path: "锁住" });
    expect(primaryActionKey(finding({ kind: "unreadable-folder", paths: ["锁住"] }))).toBe("checkup.action.reveal");
    expect(fixAction(finding({ kind: "unreadable-folder", paths: ["锁住"] }))).toBeNull();
  });

  it("没有涉及页的发现不给主按钮（绝不给一个点了什么都不发生的按钮）", () => {
    expect(primaryAction(finding({ kind: "orphan", paths: [] }))).toBeNull();
  });

  it("「修复经预览」只认 Rust 给的 fix 字段", () => {
    expect(fixAction(finding({ fix: "rescan" }))).toEqual({ kind: "rescan" });
    // 前端不自己决定谁可以修：Rust 没给 fix 就没有修复按钮，哪怕种类看起来"应该能修"。
    expect(fixAction(finding({ kind: "orphan", fix: null }))).toBeNull();
  });
});

describe("体检卡的动态文案", () => {
  it("每一类发现的标题与说明在中英文案里都在场", () => {
    for (const kind of KINDS) {
      for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
        expect(lookup(bundle, `checkup.card.${kind}.title`), `缺 ${kind}.title`).toBeTruthy();
        expect(lookup(bundle, `checkup.card.${kind}.desc`), `缺 ${kind}.desc`).toBeTruthy();
        // 「怎么办」不是可选的：体检刻意不自动修，缺了这一句这张卡就只是在指责用户。
        expect(lookup(bundle, `checkup.card.${kind}.todo`), `缺 ${kind}.todo`).toBeTruthy();
      }
    }
  });

  it("每一类主按钮的文案 key 都能查到", () => {
    for (const kind of KINDS) {
      const key = primaryActionKey(finding({ kind }));
      for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
        expect(lookup(bundle, key), `缺 ${key}`).toBeTruthy();
      }
    }
  });
});
