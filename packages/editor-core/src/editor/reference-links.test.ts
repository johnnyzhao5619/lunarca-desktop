// 未解析引用式链接的去链接化（PRD-16-005）。判据与**用哪个视图看**无关，故这组用例
// 直接打在 state 级纯函数上——Live 与源码两模式消费的是同一份装饰。

import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { buildUnresolvedLinkDecorations } from "./reference-links";

function mdState(doc: string): EditorState {
  return EditorState.create({ doc, extensions: [markdown({ base: markdownLanguage })] });
}

/** 「未解析引用式链接」标记覆盖的区间。 */
function unresolvedRanges(doc: string): Array<{ from: number; to: number }> {
  const state = mdState(doc);
  const set = buildUnresolvedLinkDecorations(state, [{ from: 0, to: state.doc.length }]);
  const out: Array<{ from: number; to: number }> = [];
  const iter = set.iter();
  while (iter.value) {
    if (iter.value.spec.class === "luna-md-link--unresolved") {
      out.push({ from: iter.from, to: iter.to });
    }
    iter.next();
  }
  return out;
}

describe("解析不到定义的引用式链接一律去链接色", () => {
  const dead = [
    ["简写式", "反馈数据路由更新及测试结果 [4]。在此期间"],
    ["完整式", "以避免阻碍 SQ 团队的测试和发布流程 [1][4]。这需要"],
    ["折叠式", "折叠 [1][] 引用"],
    ["下标写法", "数组取值 list[1] 不是引用"],
    // 真机现场：AI 生成的会议纪要里的人名被着成链接色，点下去什么也不发生。
    ["人名角标", "会议由 [Eric Mcelroy] 主持"],
  ] as const;

  for (const [name, doc] of dead) {
    it(`${name}：挂上未解析标记`, () => {
      expect(unresolvedRanges(doc).length).toBeGreaterThan(0);
    });
  }

  it("有对应定义时仍是真链接：不挂未解析标记", () => {
    // 反向验证：判据不能把合法的引用式链接一并打死。
    expect(unresolvedRanges("[1]: https://example.com\n\n正文引用 [1] 在此")).toEqual([]);
  });

  it("行内链接不受影响", () => {
    expect(unresolvedRanges("真链接 [标题](../Notes/a.md) 在此")).toEqual([]);
  });
});
