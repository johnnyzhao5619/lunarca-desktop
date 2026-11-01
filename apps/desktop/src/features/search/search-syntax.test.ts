import { describe, expect, it } from "vitest";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import { SEARCH_SYNTAX_HINT_IDS } from "./search-syntax";

describe("搜索语法速查 i18n 契约", () => {
  it("每个已支持能力在中英文里都有样例与说明", () => {
    for (const bundle of [zhWorkspace, enWorkspace]) {
      for (const id of SEARCH_SYNTAX_HINT_IDS) {
        expect(bundle.search.syntaxSamples[id]).not.toHaveLength(0);
        expect(bundle.search.syntax[id]).not.toHaveLength(0);
      }
    }
  });

  it("英文界面不回退为中文样例", () => {
    for (const id of SEARCH_SYNTAX_HINT_IDS) {
      expect(enWorkspace.search.syntaxSamples[id]).not.toMatch(/\p{Script=Han}/u);
    }
  });
});
