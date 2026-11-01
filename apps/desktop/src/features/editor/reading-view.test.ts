// 阅读模式纯逻辑锁定（PRD-06-017）：frontmatter 拆分/属性提取 + wiki-link/#tag 切分。

import { describe, expect, it } from "vitest";
import {
  frontmatterEntries,
  splitFrontmatter,
  splitReadingInline,
} from "./reading-view";

describe("splitFrontmatter", () => {
  it("splits a leading YAML block from the body", () => {
    const { frontmatter, body } = splitFrontmatter("---\ntags:\n  - a\n---\n# Hi\n");
    expect(frontmatter).toBe("tags:\n  - a");
    expect(body).toBe("# Hi\n");
  });

  it("returns the whole content when there is no frontmatter", () => {
    expect(splitFrontmatter("# Hi")).toEqual({ frontmatter: null, body: "# Hi" });
    // `---` 出现在正文中不是 frontmatter。
    expect(splitFrontmatter("a\n---\nb").frontmatter).toBeNull();
  });

  it("rejects an unclosed fence", () => {
    expect(splitFrontmatter("---\ntags: x\n").frontmatter).toBeNull();
  });
});

describe("frontmatterEntries", () => {
  it("extracts top-level pairs and folds list items into values", () => {
    expect(frontmatterEntries("title: Hello\ntags:\n  - a\n  - b\ncount: 3")).toEqual([
      { key: "title", value: "Hello" },
      { key: "tags", value: "a, b" },
      { key: "count", value: "3" },
    ]);
  });

  it("ignores non key-value lines", () => {
    expect(frontmatterEntries("just text\nkey: v")).toEqual([{ key: "key", value: "v" }]);
  });

  it("hides the luna machine namespace including its nested list items", () => {
    // 真机第八轮：产物页的 `luna.ai_tags` 列表项被浅解析串成「luna: <标签汤>」。
    // 隐藏键的缩进子行不得回流进上一个可见键的值里。
    const yaml =
      "title: Hello\nluna:\n  schema: 1\n  ai_tags:\n    - cutover-迁移\n    - 运维/计划\ntags:\n  - mine";
    expect(frontmatterEntries(yaml)).toEqual([
      { key: "title", value: "Hello" },
      { key: "tags", value: "mine" },
    ]);
  });
});

describe("splitReadingInline", () => {
  it("splits wiki links with aliases", () => {
    expect(splitReadingInline("see [[Note A|alias]] and [[B]]")).toEqual([
      { kind: "text", text: "see " },
      { kind: "wiki", target: "Note A", label: "alias" },
      { kind: "text", text: " and " },
      { kind: "wiki", target: "B", label: "B" },
    ]);
  });

  it("splits inline tags with the same rules as the editor decorations", () => {
    expect(splitReadingInline("todo #项目/luna now")).toEqual([
      { kind: "text", text: "todo " },
      { kind: "tag", tag: "项目/luna" },
      { kind: "text", text: " now" },
    ]);
    // 纯数字不是标签；非行首/空白前缀不触发。
    expect(splitReadingInline("a#x #123")).toEqual([{ kind: "text", text: "a#x #123" }]);
  });

  it("passes plain text through untouched", () => {
    expect(splitReadingInline("plain text")).toEqual([{ kind: "text", text: "plain text" }]);
  });
});
