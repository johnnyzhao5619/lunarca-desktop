import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { syntaxTree } from "@codemirror/language";
import {
  buildTagDecorations,
  buildTaskDecorations,
  scanLineForTags,
  taskToggleChangeAt,
} from "./markdown-decorations";

function mdState(doc: string): EditorState {
  return EditorState.create({
    doc,
    extensions: [markdown({ base: markdownLanguage })],
  });
}

function fullRange(state: EditorState) {
  return [{ from: 0, to: state.doc.length }];
}

describe("GFM 方言解析（PRD-06-016 基线：表格/任务/删除线）", () => {
  it("识别表格表头与分隔行节点", () => {
    const state = mdState("| 列A | 列B |\n| --- | --- |\n| 1 | 2 |");
    const names = new Set<string>();
    syntaxTree(state).iterate({
      enter(node) {
        names.add(node.name);
      },
    });
    expect(names).toContain("Table");
    expect(names).toContain("TableHeader");
    expect(names).toContain("TableDelimiter");
  });

  it("识别任务列表与删除线节点", () => {
    const state = mdState("- [ ] 待办\n- [x] 已完成\n~~删除~~");
    const names = new Set<string>();
    syntaxTree(state).iterate({
      enter(node) {
        names.add(node.name);
      },
    });
    expect(names).toContain("TaskMarker");
    expect(names).toContain("Strikethrough");
  });
});

describe("任务复选框装饰与切换", () => {
  it("为每个 TaskMarker 生成替换装饰", () => {
    const state = mdState("- [ ] 甲\n- [x] 乙\n- 普通项");
    expect(buildTaskDecorations(state, fullRange(state)).size).toBe(2);
  });

  it("taskToggleChangeAt 在标记处翻转 [ ]/[x]", () => {
    const doc = "- [ ] 甲乙丙丁\n- [x] 乙";
    const state = mdState(doc);
    const unchecked = taskToggleChangeAt(state, doc.indexOf("[ ]") + 1);
    expect(unchecked?.insert).toBe("[x]");
    const checked = taskToggleChangeAt(state, doc.indexOf("[x]") + 1);
    expect(checked?.insert).toBe("[ ]");
    // 远离标记处（正文文字上，超出 ±1 相邻容差）不产生切换。
    expect(taskToggleChangeAt(state, doc.indexOf("丙"))).toBeNull();
  });
});

describe("内联 #tag 装饰", () => {
  it("装饰正文标签，跳过代码块内的 #", () => {
    const state = mdState("#工作 正文\n```\n#in-fence\n```\n尾行 #复盘");
    expect(buildTagDecorations(state, fullRange(state)).size).toBe(2);
  });
});

describe("scanLineForTags（与 Rust extract_inline_tags 同构）", () => {
  it("识别行首/空白后的 #tag，含嵌套与 CJK", () => {
    const matches = scanLineForTags("#工作 中间 #project/luna 与 #re-check_2");
    expect(matches.map((m) => m.tag)).toEqual(["工作", "project/luna", "re-check_2"]);
  });

  it("返回的偏移覆盖 `#` 到标签体末尾", () => {
    const [m] = scanLineForTags("见 #tag 后文");
    expect(m.from).toBe(2);
    expect(m.to).toBe(6);
  });

  it("拒绝标题、纯数字与词中 #", () => {
    expect(scanLineForTags("# 标题")).toEqual([]);
    expect(scanLineForTags("序号 #123")).toEqual([]);
    expect(scanLineForTags("词中a#b")).toEqual([]);
  });

  it("跳过行内 code span，尾部斜杠不计入标签体", () => {
    expect(scanLineForTags("`#code` 之外 #real/").map((m) => m.tag)).toEqual(["real"]);
    const [m] = scanLineForTags("#a/");
    expect(m.to).toBe(2);
  });

  it("拒绝以斜杠开头的标签体", () => {
    expect(scanLineForTags("#/oops")).toEqual([]);
  });
});
