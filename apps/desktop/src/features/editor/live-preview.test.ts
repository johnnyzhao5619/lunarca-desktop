import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import {
  BulletWidget,
  buildLivePreviewDecorations,
  CalloutLabelWidget,
  boundaryInside,
  caretTouches,
  listMarkAtoms,
  revealsSyntax,
  selectionShape,
} from "./live-preview";
import { ImageEmbedWidget } from "./image-embed";

function mdState(doc: string, cursor = 0): EditorState {
  return EditorState.create({
    doc,
    selection: { anchor: cursor },
    extensions: [markdown({ base: markdownLanguage })],
  });
}

function decoCount(state: EditorState): number {
  return buildLivePreviewDecorations(state, [{ from: 0, to: state.doc.length }]).size;
}

function decorationRanges(state: EditorState): Array<{ from: number; to: number }> {
  const set = buildLivePreviewDecorations(state, [{ from: 0, to: state.doc.length }]);
  const ranges: Array<{ from: number; to: number }> = [];
  const iter = set.iter();
  while (iter.value) {
    ranges.push({ from: iter.from, to: iter.to });
    iter.next();
  }
  return ranges;
}

/** 装饰集中就地图片 widget 的数量（供图片渲染/源码显隐断言）。 */
function imageWidgetCount(state: EditorState): number {
  const set = buildLivePreviewDecorations(state, [{ from: 0, to: state.doc.length }], {
    notePath: "Notes/test.md",
  });
  let count = 0;
  const iter = set.iter();
  while (iter.value) {
    if (iter.value.spec.widget instanceof ImageEmbedWidget) {
      count += 1;
    }
    iter.next();
  }
  return count;
}

describe("触及判定（光标点 / 非空选区端点）", () => {
  it("光标点与范围相触判定（含端点）；非空选区不算光标点", () => {
    const state = mdState("abcdef", 3);
    expect(caretTouches(state, 0, 3)).toBe(true);
    expect(caretTouches(state, 3, 5)).toBe(true);
    expect(caretTouches(state, 4, 6)).toBe(false);
    expect(caretTouches(selState("abcdef", 0, 6), 0, 6)).toBe(false);
  });

  /**
   * 非空选区只在**端点落在元素内部**时才算触及行内元素：整个包住（三击整行、选中
   * 整个粗体词）是复制 / 格式化的意图，不还原；端点停在被藏起的标记里才必须还原。
   */
  it("非空选区：端点在元素内部才算，包住整个元素不算", () => {
    const doc = "看 **加粗** 与后文";
    const bold = { from: doc.indexOf("**"), to: doc.lastIndexOf("**") + 2 };
    expect(boundaryInside(selState(doc, bold.from, bold.to), bold.from, bold.to)).toBe(false);
    expect(boundaryInside(selState(doc, 0, doc.length), bold.from, bold.to)).toBe(false);
    expect(boundaryInside(selState(doc, 0, bold.from + 3), bold.from, bold.to)).toBe(true);
    expect(boundaryInside(selState(doc, bold.to - 1, doc.length), bold.from, bold.to)).toBe(true);
    expect(boundaryInside(mdState(doc, bold.from + 3), bold.from, bold.to)).toBe(false);
  });

  it("选区形态：光标点 / 单行内 / 跨行；尾端恰在行首不算跨进那一行", () => {
    const doc = "第一行\n第二行\n第三行";
    expect(selectionShape(mdState(doc, 2))).toBe("caret");
    expect(selectionShape(selState(doc, 0, 3))).toBe("inline");
    expect(selectionShape(selState(doc, 0, 4))).toBe("inline"); // 三击整行：含换行符
    expect(selectionShape(selState(doc, 0, 5))).toBe("block");
    expect(revealsSyntax(selState(doc, 0, 5))).toBe(false);
  });
});

function selState(doc: string, anchor: number, head: number): EditorState {
  return EditorState.create({
    doc,
    selection: { anchor, head },
    extensions: [markdown({ base: markdownLanguage })],
  });
}

describe("Live Preview 选区策略（PRD-06-011，2026-09-04 裁决）", () => {
  it("跨行选区不还原任何语法：标题 # 与行内 ** 全部保持隐藏", () => {
    const doc = "# 标题\n\n看 **加粗** 与后文";
    const away = mdState(doc, doc.length);
    const block = selState(doc, 0, doc.length);
    expect(decoCount(block)).toBe(decoCount(away));
  });

  it("单行内的非空选区：端点落在粗体内部才还原 `**`，包住整个粗体不还原", () => {
    const doc = "看 **加粗** 与后文";
    const away = mdState(doc, doc.length);
    const inside = selState(doc, doc.indexOf("加粗"), doc.indexOf("加粗") + 2);
    expect(decoCount(away) - decoCount(inside)).toBe(2);
    const whole = selState(doc, doc.indexOf("**"), doc.lastIndexOf("**") + 2);
    expect(decoCount(whole)).toBe(decoCount(away));
  });

  it("三击整行（选区尾落在下一行行首）：本行与下一行的块级标记都不还原", () => {
    const doc = "# 甲\n# 乙";
    const secondLine = doc.indexOf("# 乙");
    expect(decorationRanges(selState(doc, 0, secondLine))).toEqual([
      { from: 0, to: 2 },
      { from: secondLine, to: secondLine + 2 },
    ]);
  });

  it("三击整个列表项：`**` 与圆点都保持渲染", () => {
    const doc = "- **甲** 乙\n- 丙";
    const away = mdState(doc, doc.length);
    const wholeItem = selState(doc, 0, doc.indexOf("- 丙"));
    expect(decoCount(wholeItem)).toBe(decoCount(away));
  });

  it("无序圆点常驻：光标落在该行也不还原，且替换区间登记为原子区间", () => {
    const doc = "- 甲\n- [ ] 乙";
    const onLine = mdState(doc, 2);
    const set = buildLivePreviewDecorations(onLine, [{ from: 0, to: doc.length }]);
    const atoms: Array<{ from: number; to: number }> = [];
    const iter = listMarkAtoms(set).iter();
    while (iter.value) {
      atoms.push({ from: iter.from, to: iter.to });
      iter.next();
    }
    // `- ` 圆点 + 任务项的 `- ` 各一段；任务框本身由 markdown-decorations 的复选框负责。
    expect(atoms).toEqual([
      { from: 0, to: 2 },
      { from: 4, to: 6 },
    ]);
  });
});

describe("Live Preview 标记隐藏（PRD-06-011 Obsidian 对标）", () => {
  it("光标在别处时隐藏标题 # 标记；光标落在标题行时还原", () => {
    const doc = "# 标题\n\n正文段落";
    const away = mdState(doc, doc.length); // 光标在正文
    const onHeading = mdState(doc, 2); // 光标在标题行
    expect(decoCount(away)).toBeGreaterThan(decoCount(onHeading));
  });

  it("行内加粗标记按元素范围显隐", () => {
    const doc = "看 **加粗** 与后文尾部";
    const away = mdState(doc, doc.length);
    const inside = mdState(doc, doc.indexOf("加粗") + 1);
    // 远离：两个 EmphasisMark 都隐藏；光标进入元素：全部还原。
    expect(decoCount(away) - decoCount(inside)).toBe(2);
  });

  it("wiki-link 折叠括号；别名形式只留别名", () => {
    const plain = mdState("见 [[目标]] 结束", 0);
    const decos = buildLivePreviewDecorations(plain, [
      { from: 0, to: plain.doc.length },
    ]);
    expect(decos.size).toBe(2); // [[ 与 ]]
    const alias = mdState("见 [[目标|别名]] 结束", 0);
    const aliasDecos = buildLivePreviewDecorations(alias, [
      { from: 0, to: alias.doc.length },
    ]);
    expect(aliasDecos.size).toBe(2); // `[[目标|` 与 `]]`
  });

  it("代码块内的 wiki-link 不折叠，但未触及的围栏仍按 Live Preview 隐藏", () => {
    const doc = "```\n[[目标]]\n```";
    // 只有开/关围栏投影；`[[目标]]` 本身绝不能变成可点击 wiki-link 或被折叠。
    expect(decoCount(mdState(doc, 0))).toBe(2);
  });

  it("未触及围栏头时隐藏 ```，但保留语言 Info String 与代码正文", () => {
    const doc = "```javascript\nconst value = 1;\n```\n\n正文";
    const state = mdState(doc, doc.length);
    const openingMarkerEnd = 3;
    const closingStart = doc.indexOf("```", openingMarkerEnd);
    const ranges = decorationRanges(state);
    expect(ranges).toContainEqual({ from: 0, to: openingMarkerEnd });
    expect(ranges).toContainEqual({ from: closingStart, to: closingStart + 3 });
    expect(ranges).not.toContainEqual({ from: 3, to: "```javascript".length });
  });

  it("光标或选区触及围栏头时还原完整围栏源码", () => {
    const doc = "```javascript\nconst value = 1;\n```\n\n正文";
    const onHeader = mdState(doc, 2);
    const ranges = decorationRanges(onHeader);
    expect(ranges).not.toContainEqual({ from: 0, to: 3 });
    // 结束围栏仍未触及，保持隐藏，符合按行显隐而非整块回退的 Live Preview 心智。
    const closingStart = doc.indexOf("```", 3);
    expect(ranges).toContainEqual({ from: closingStart, to: closingStart + 3 });
  });

  it("未闭合围栏不会把最后一行代码误判为结束围栏", () => {
    const doc = "```js\nconsole.log('unfinished')";
    const state = mdState(doc, doc.length);
    const codeStart = doc.indexOf("console");
    expect(decorationRanges(state)).not.toContainEqual({
      from: codeStart,
      to: doc.length,
    });
  });

  it("水平分隔线在光标离开时替换为渲染件", () => {
    const doc = "上文\n\n---\n\n下文";
    const away = mdState(doc, 0);
    const onHr = mdState(doc, doc.indexOf("---") + 1);
    expect(decoCount(away) - decoCount(onHr)).toBe(1);
  });

  it("引用块每行获得左边条 line 装饰且引用标记折叠", () => {
    const doc = "> 引用一\n> 引用二\n\n正文";
    const away = mdState(doc, doc.length);
    // 2 个 QuoteMark 隐藏 + 2 个引用行 class = 4。
    expect(decoCount(away)).toBe(4);
  });

  it("无序列表圆点替换，光标所在行也不还原（圆点常驻）", () => {
    const doc = "- 甲\n- 乙";
    const onSecond = mdState(doc, doc.indexOf("乙")); // 光标在第二行
    expect(decoCount(onSecond)).toBe(2);
  });
});

describe("Live Preview 图片就地渲染（PRD-06-011/013，光标行显隐）", () => {
  const doc = "前文\n\n![](assets/pic.png)\n\n后文";
  const imageLine = doc.indexOf("![");

  it("光标离开图片行时渲染图片 widget", () => {
    // 光标在首行（非图片行）→ 图片就地渲染。
    expect(imageWidgetCount(mdState(doc, 0))).toBe(1);
  });

  it("光标落在图片行时还原源码（不渲染 widget）", () => {
    // 光标在图片行 → 显示源码可编辑链接（Obsidian 对标）。
    expect(imageWidgetCount(mdState(doc, imageLine + 2))).toBe(0);
  });

  it("远程 http(s) 图片不就地渲染（离线纪律）", () => {
    const remote = "![](https://example.com/x.png)";
    expect(imageWidgetCount(mdState(remote, remote.length))).toBe(0);
  });
});

describe("Live Preview 标注块（PRD-06-011，Obsidian callout 对标）", () => {
  /** 装饰集中的行装饰 `data-callout*` 属性序列（按行序）。 */
  function calloutLineAttrs(state: EditorState): Array<[string, string]> {
    const set = buildLivePreviewDecorations(state, [{ from: 0, to: state.doc.length }]);
    const out: Array<[string, string]> = [];
    const iter = set.iter();
    while (iter.value) {
      const attrs = iter.value.spec.attributes as Record<string, string> | undefined;
      if (attrs?.["data-callout"]) {
        out.push([attrs["data-callout"], attrs["data-callout-pos"]]);
      }
      iter.next();
    }
    return out;
  }

  /** 装饰集中类型标签 widget 的文本（无则 null）。无头环境不建 DOM，只看 widget 本身。 */
  function calloutLabel(state: EditorState): string | null {
    const set = buildLivePreviewDecorations(state, [{ from: 0, to: state.doc.length }]);
    const iter = set.iter();
    while (iter.value) {
      const widget = iter.value.spec.widget;
      if (widget instanceof CalloutLabelWidget) {
        return widget.label;
      }
      iter.next();
    }
    return null;
  }

  it("整块每行带色调与位置，首尾各自成角", () => {
    const doc = "> [!warning] 注意\n> 正文一\n> 正文二";
    expect(calloutLineAttrs(mdState(doc, 0))).toEqual([
      ["warning", "head"],
      ["warning", "body"],
      ["warning", "tail"],
    ]);
  });

  it("单行标注块是 solo（不会既当头又当尾）", () => {
    expect(calloutLineAttrs(mdState("> [!tip] 就一行", 0))).toEqual([["success", "solo"]]);
  });

  it("光标不在头行时把 [!type] 收成类型标签，落回该行即还原源码", () => {
    const doc = "正文\n\n> [!danger] 危险\n> 说明";
    expect(calloutLabel(mdState(doc, 0))).toBe("DANGER");
    expect(calloutLabel(mdState(doc, doc.indexOf("危险")))).toBeNull();
  });

  it("普通引用块不受影响（仍是引用左边条，无标注属性）", () => {
    expect(calloutLineAttrs(mdState("> 普通引用\n> 第二行", 0))).toEqual([]);
  });
});

describe("引用式链接：解析不到定义就不折叠方括号", () => {
  // CommonMark 把 `[1]` 当引用式链接，而 lezer **无论有没有对应定义都产出同一个
  // `Link` 节点**（已用真实 parser 实测）。旧实现无条件隐藏 LinkMark，于是死角标被
  // 藏掉方括号，看起来是链接、点下去什么也不发生——正是 PRD-16-005 点名禁止的
  //「容易显示、难以点开」。去链接**色**归 reference-links.ts（两模式共用，见该文件）。
  const dead = [
    ["简写式", "反馈数据路由更新及测试结果 [4]。在此期间"],
    ["完整式", "以避免阻碍 SQ 团队的测试和发布流程 [1][4]。这需要"],
    ["折叠式", "折叠 [1][] 引用"],
    ["下标写法", "数组取值 list[1] 不是引用"],
  ] as const;

  for (const [name, doc] of dead) {
    it(`${name}：不折叠方括号`, () => {
      const state = mdState(doc, doc.length);
      const bracket = doc.indexOf("[");
      // 一条 hide 装饰都不该盖住方括号（盖住 = 用户看不见它其实是字面文本）。
      const hidden = decorationRanges(state).filter(
        (r) => r.from <= bracket && r.to > bracket && r.to - r.from <= 2,
      );
      expect(hidden).toEqual([]);
    });
  }

  it("行内链接不受影响：`[文本](路径)` 照旧折叠成纯文本", () => {
    const doc = "真链接 [标题](../Notes/a.md) 在此";
    const state = mdState(doc, doc.length);
    // 折叠仍在发生（`](../Notes/a.md)` 被隐藏）。
    expect(decorationRanges(state).length).toBeGreaterThan(0);
  });
});

/**
 * 圆点的**层级**（PRD-06-017 两面同源）：字形不在这里定，`data-depth` 只声明层级，
 * 形状交给 `--editor-list-marker-*`——阅读面的 `::marker` 消费同一组 Token。
 */
describe("无序列表圆点按层级区分", () => {
  function bulletDepths(doc: string): number[] {
    const state = mdState(doc, doc.length);
    const set = buildLivePreviewDecorations(state, [{ from: 0, to: state.doc.length }]);
    const out: number[] = [];
    const iter = set.iter();
    while (iter.value) {
      const widget: unknown = iter.value.spec.widget;
      if (widget instanceof BulletWidget) {
        out.push(widget.depth);
      }
      iter.next();
    }
    return out;
  }

  it("多行嵌套：层级随缩进递进", () => {
    expect(bulletDepths("- 一层\n  - 二层\n    - 三层\n")).toEqual([1, 2, 3]);
  });

  it("一行内嵌套（`* * 文字`）：同一行的两个标记分属两层", () => {
    expect(bulletDepths("* * 离线信号比对\n")).toEqual([1, 2]);
  });

  it("层级封顶 3（同浏览器默认阶：第 3 层起不再变形状）", () => {
    expect(bulletDepths("* * * * * * 分阶段迁移\n")).toEqual([1, 2, 3, 3, 3, 3]);
  });

  it("同层级的 widget 判等（CM6 据此复用 DOM，不同层级不得混用）", () => {
    expect(new BulletWidget(2).eq(new BulletWidget(2))).toBe(true);
    expect(new BulletWidget(1).eq(new BulletWidget(2))).toBe(false);
  });
});
