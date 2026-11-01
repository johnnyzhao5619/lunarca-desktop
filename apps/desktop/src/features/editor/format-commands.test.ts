import { describe, expect, it } from "vitest";
import { EditorState, type ChangeSpec } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import {
  calloutSpec,
  codeBlockSpec,
  formatStateAt,
  horizontalRuleSpec,
  inlineToggleSpec,
  setHeadingChanges,
  toggleLineBlockChanges,
} from "./format-commands";

function mdState(doc: string, anchor = 0, head = anchor): EditorState {
  return EditorState.create({
    doc,
    selection: { anchor, head },
    extensions: [markdown({ base: markdownLanguage })],
  });
}

function applyChanges(state: EditorState, changes: ChangeSpec | ChangeSpec[]): string {
  return state.update({ changes }).state.doc.toString();
}

describe("行内标记真 toggle（Obsidian/Typora 对标）", () => {
  it("普通选区包裹加粗", () => {
    const doc = "你好世界";
    const state = mdState(doc, 0, 2);
    const spec = inlineToggleSpec(state, 0, 2, "strong");
    expect(applyChanges(state, spec.changes)).toBe("**你好**世界");
    expect(spec.anchor).toBe(2);
    expect(spec.head).toBe(4);
  });

  it("光标在加粗元素内再次触发 → 去加粗（真 toggle）", () => {
    const doc = "前 **粗体** 后";
    const inside = doc.indexOf("粗体") + 1;
    const state = mdState(doc, inside);
    const spec = inlineToggleSpec(state, inside, inside, "strong");
    expect(applyChanges(state, spec.changes)).toBe("前 粗体 后");
  });

  it("删除线与行内代码同语义", () => {
    const state = mdState("abc", 0, 3);
    expect(applyChanges(state, inlineToggleSpec(state, 0, 3, "del").changes)).toBe("~~abc~~");
    expect(applyChanges(state, inlineToggleSpec(state, 0, 3, "code").changes)).toBe("`abc`");
  });

  it("空选区插入标记对、光标置中", () => {
    const state = mdState("x", 1);
    const spec = inlineToggleSpec(state, 1, 1, "strong");
    expect(applyChanges(state, spec.changes)).toBe("x****");
    expect(spec.anchor).toBe(3);
  });
});

describe("标题设置/切换（setHeadingChanges）", () => {
  it("正文行设为 H2；已是 H2 再设 → 还原正文（toggle）", () => {
    const plain = mdState("标题文本", 2);
    expect(applyChanges(plain, setHeadingChanges(plain, 2))).toBe("## 标题文本");
    const h2 = mdState("## 标题文本", 3);
    expect(applyChanges(h2, setHeadingChanges(h2, 2))).toBe("标题文本");
  });

  it("跨级切换直接替换前缀；列表前缀被替换", () => {
    const h1 = mdState("# 标题", 2);
    expect(applyChanges(h1, setHeadingChanges(h1, 3))).toBe("### 标题");
    const li = mdState("- 项目", 2);
    expect(applyChanges(li, setHeadingChanges(li, 1))).toBe("# 项目");
  });

  it("level 0 显式还原正文", () => {
    const h4 = mdState("#### 深层", 2);
    expect(applyChanges(h4, setHeadingChanges(h4, 0))).toBe("深层");
  });
});

describe("行级块 toggle（列表/任务/引用）", () => {
  it("多行统一转无序列表；全部已是列表再触发 → 还原", () => {
    const doc = "甲\n乙";
    const state = mdState(doc, 0, doc.length);
    const next = applyChanges(state, toggleLineBlockChanges(state, "bullet"));
    expect(next).toBe("- 甲\n- 乙");
    const listed = mdState(next, 0, next.length);
    expect(applyChanges(listed, toggleLineBlockChanges(listed, "bullet"))).toBe("甲\n乙");
  });

  it("有序列表自动编号；空行跳过", () => {
    const doc = "甲\n\n乙";
    const state = mdState(doc, 0, doc.length);
    expect(applyChanges(state, toggleLineBlockChanges(state, "ordered"))).toBe("1. 甲\n\n2. 乙");
  });

  it("任务列表与无序列表互斥转换", () => {
    const state = mdState("- 项目", 0, 4);
    expect(applyChanges(state, toggleLineBlockChanges(state, "task"))).toBe("- [ ] 项目");
    const task = mdState("- [ ] 项目", 0, 4);
    expect(applyChanges(task, toggleLineBlockChanges(task, "bullet"))).toBe("- 项目");
  });

  it("引用 toggle", () => {
    const state = mdState("一句话", 1);
    expect(applyChanges(state, toggleLineBlockChanges(state, "quote"))).toBe("> 一句话");
    const quoted = mdState("> 一句话", 3);
    expect(applyChanges(quoted, toggleLineBlockChanges(quoted, "quote"))).toBe("一句话");
  });
});

describe("块插入规格", () => {
  it("选区包成代码块", () => {
    const doc = "let x = 1;";
    const state = mdState(doc, 0, doc.length);
    const spec = codeBlockSpec(state);
    expect(spec.insert).toBe("```\nlet x = 1;\n```");
    expect(spec.anchor).toBe(4);
  });

  it("空选区非空行 → 下一行插入空代码块", () => {
    const doc = "正文";
    const state = mdState(doc, doc.length);
    const spec = codeBlockSpec(state);
    expect(spec.insert.startsWith("\n```")).toBe(true);
    expect(spec.from).toBe(doc.length);
  });

  it("分隔线：空行就地、非空行另起空行", () => {
    const empty = mdState("", 0);
    expect(horizontalRuleSpec(empty).insert).toBe("---\n");
    const busy = mdState("正文", 2);
    expect(horizontalRuleSpec(busy).insert).toBe("\n\n---\n");
  });
});

describe("光标处格式态检测（formatStateAt）", () => {
  it("加粗/斜体/删除线/行内代码内检测为激活", () => {
    const doc = "**粗** *斜* ~~删~~ `码`";
    expect(formatStateAt(mdState(doc, doc.indexOf("粗") + 1)).strong).toBe(true);
    expect(formatStateAt(mdState(doc, doc.indexOf("斜") + 1)).em).toBe(true);
    expect(formatStateAt(mdState(doc, doc.indexOf("删") + 1)).del).toBe(true);
    expect(formatStateAt(mdState(doc, doc.indexOf("码") + 1)).code).toBe(true);
    const outside = formatStateAt(mdState(doc, 0));
    expect(outside.em).toBe(false);
  });

  it("行级态：标题级别/列表/任务/引用", () => {
    expect(formatStateAt(mdState("## 标题", 3)).heading).toBe(2);
    expect(formatStateAt(mdState("- 项", 2)).bullet).toBe(true);
    expect(formatStateAt(mdState("1. 项", 3)).ordered).toBe(true);
    expect(formatStateAt(mdState("- [ ] 项", 7)).task).toBe(true);
    expect(formatStateAt(mdState("> 引", 2)).quote).toBe(true);
  });
});

describe("标注块插入（calloutSpec，PRD-06-011）", () => {
  it("空选区插入待填骨架，光标落在类型标记之后", () => {
    const state = mdState("", 0);
    const spec = calloutSpec(state);
    expect(spec.insert).toBe("> [!note] \n> ");
    expect(spec.anchor).toBe("> [!note] ".length);
    expect(spec.head).toBe(spec.anchor);
  });

  it("非空行先另起一段再插入（绝不把标注挤进正在写的那一行）", () => {
    const doc = "已有正文";
    const spec = calloutSpec(mdState(doc, doc.length));
    expect(spec.insert.startsWith("\n\n> [!note] ")).toBe(true);
    expect(spec.from).toBe(doc.length);
  });

  it("选区逐行加引用前缀，整段收进标注块", () => {
    const doc = "第一行\n第二行";
    const state = EditorState.create({
      doc,
      selection: { anchor: 0, head: doc.length },
      extensions: [markdown({ base: markdownLanguage })],
    });
    expect(calloutSpec(state).insert).toBe("> [!note] \n> 第一行\n> 第二行");
  });
});
