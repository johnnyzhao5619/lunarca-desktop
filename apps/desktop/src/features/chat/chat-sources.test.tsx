import { describe, expect, it, vi } from "vitest";
import { isValidElement } from "react";
import { chatSourcesLayout, citationTextRenderer, sourceHoverTitle } from "./ChatSources";
import type { RetrievalSourceDto } from "../../ipc/bindings";

/** 取词桩：只回声 key 与参数，断言看的是「取了哪一句」而不是具体译文。 */
const t = (key: string, params?: Record<string, unknown>) =>
  params ? `${key}:${JSON.stringify(params)}` : key;

function source(index: number, title: string, draft = false): RetrievalSourceDto {
  return {
    index,
    path: `Notes/${title}.md`,
    title,
    note_type: "note",
    source_material: false,
    draft,
    snippet: `${title} 的摘录`,
    anchor: { heading_path: [], quote: "", prefix: "", suffix: "", line_hint: 1, source_hash: "" },
  };
}

describe("citationTextRenderer", () => {
  it("无出处时不接管渲染（返回 null 即走默认原样文本）", () => {
    const render = citationTextRenderer([], vi.fn(), t);
    expect(render("依据 [1] 如此", 0)).toBeNull();
  });

  it("已下发的编号变成可点角标", () => {
    const onOpen = vi.fn();
    const render = citationTextRenderer([source(1, "甲")], onOpen, t);
    const node = render("结论如此 [1]。", 0);
    expect(isValidElement(node)).toBe(true);
  });

  it("模型编出的不存在编号不做成角标——点了什么都没有的按钮正是过度信任的来源", () => {
    const render = citationTextRenderer([source(1, "甲")], vi.fn(), t);
    // 只有 [9]：一个都认不出，交回默认渲染，`[9]` 原样呈现。
    expect(render("依据 [9] 如此", 0)).toBeNull();
  });

  it("混合时只角标化认得的那个", () => {
    const render = citationTextRenderer([source(2, "乙")], vi.fn(), t);
    const node = render("先 [9] 后 [2]", 0);
    expect(isValidElement(node)).toBe(true);
    const children = (node as { props: { children: unknown[] } }).props.children;
    // `[9]` 留在文本里、`[2]` 成为元素——如实呈现模型说了什么。
    expect(children.some((c) => typeof c === "string" && c.includes("[9]"))).toBe(true);
    expect(children.some((c) => isValidElement(c))).toBe(true);
  });

  it("不含方括号的文本走快速路径", () => {
    const render = citationTextRenderer([source(1, "甲")], vi.fn(), t);
    expect(render("普通文本", 0)).toBeNull();
  });
});

describe("角标与芯片的编号空间分工", () => {
  it("角标用检索全集、芯片用被引用子集——两者混同会让流式期间的角标点不动", () => {
    // 检索到 3 篇（全集），回答只引用了第 2 篇（子集）。模型也可能提到 [1] 或 [3]，
    // 那两个角标同样必须可点：编号空间是本轮**下发过**的全部出处。
    const all = [source(1, "甲"), source(2, "乙"), source(3, "丙")];
    const render = citationTextRenderer(all, vi.fn(), t);
    for (const marker of ["[1]", "[2]", "[3]"]) {
      expect(isValidElement(render(`依据 ${marker} 如此`, 0))).toBe(true);
    }
    // 超出全集的编号仍然不做成角标。
    expect(render("依据 [4] 如此", 0)).toBeNull();
  });
});

describe("chatSourcesLayout（PRD-16-011：归档重载必须恢复出处芯片）", () => {
  it("会话运行期：透明行与芯片都在", () => {
    const retrieval = {
      scanned: 11,
      sources: [source(1, "甲")],
      include_chat: false,
      exclude_drafts: false,
      excluded: [],
      truncated: false,
    };
    expect(chatSourcesLayout(retrieval, [source(1, "甲")])).toEqual({
      transparency: true,
      chips: true,
    });
  });

  it("归档重载：透明信息没入过档，但芯片必须照常渲染", () => {
    // 反向验证：把两者并回一个 `if (!retrieval) return null`，这一条会变成 chips:false。
    expect(chatSourcesLayout(null, [source(1, "甲")])).toEqual({
      transparency: false,
      chips: true,
    });
  });

  it("两样都没有才整块不渲染", () => {
    expect(chatSourcesLayout(null, [])).toEqual({ transparency: false, chips: false });
  });
});

describe("「来自记录」标注（PRD-16-041 / ADR-086）", () => {
  it("记录态出处的 hover 前缀「来自记录」——芯片与角标共用这一份口径", () => {
    expect(sourceHoverTitle(source(1, "草稿", true), t)).toBe(
      'sources.draftSnippet:{"snippet":"草稿 的摘录"}',
    );
  });

  it("知识态出处原样给摘录，不多说一个字", () => {
    expect(sourceHoverTitle(source(1, "定稿"), t)).toBe("定稿 的摘录");
  });
});
