// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { CompletionContext, type Completion } from "@codemirror/autocomplete";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { matchSlashCommands, SLASH_TRIGGER, SLASH_COMMANDS } from "./slash-commands";
import { slashCompletions } from "./completions";
import { fullyParsed } from "./syntax-tree-test-fixture";
import { EDITOR_CORE_LOCALES } from "../i18n";


const LABELS: Record<string, string> = {
  "editor.menu.h1": "标题 1",
  "editor.table.insert": "插入表格",
  "editor.menu.aiActions": "AI 操作…",
  "editor.menu.bulletList": "无序列表",
};
const label = (key: string) => LABELS[key] ?? key;

describe("斜杠菜单触发（对标方案 B1）", () => {
  it.each([
    ["/", true], ["/表", true], ["文字 /h", true], ["、", true], ["、biao", true], ["  、", true],
    ["and/or", false], ["苹果、香蕉", false], ["/ 空格之后", false], ["文字/", false],
  ])("%s → %s", (text, triggers) => {
    expect(SLASH_TRIGGER.test(text)).toBe(triggers);
  });

  it("文案前缀最先，其次别名，再次包含；不分大小写", () => {
    const names = (q: string) => matchSlashCommands(q, (cmd) => label(cmd.labelKey)).map((cmd) => cmd.id);
    expect(names("")).toHaveLength(SLASH_COMMANDS.length);
    expect(names("表格")[0]).toBe("table");
    expect(names("TAB")[0]).toBe("table");
    expect(names("标题")[0]).toBe("h1");
    expect(names("zzz")).toEqual([]);
  });
});

describe("斜杠菜单候选", () => {
  function setup(doc: string) {
    return fullyParsed(EditorState.create({ doc, selection: { anchor: doc.length }, extensions: [markdown({ base: markdownLanguage })] }));
  }

  it("选中即删掉「/查询词」再执行；副库窗口不给 AI；代码块里不触发", () => {
    const run = vi.fn();
    const slash = { label, run, hidden: (cmd: { id: string }) => cmd.id === "ai" };
    const state = setup("段落\n/表格");
    const result = slashCompletions(new CompletionContext(state, state.doc.length, false), slash)!;
    expect(result.from).toBe(3);
    expect(result.options[0].label).toBe("插入表格");
    const parent = document.createElement("div");
    const view = new EditorView({ parent, state });
    const apply = result.options[0].apply as (view: EditorView, completion: Completion, from: number, to: number) => void;
    apply(view, result.options[0], result.from, state.doc.length);
    expect(view.state.doc.toString()).toBe("段落\n");
    expect(run).toHaveBeenCalledWith(view, expect.objectContaining({ id: "table" }));

    const all = slashCompletions(new CompletionContext(setup("/"), 1, false), slash)!;
    expect(all.options.map((o) => o.label)).not.toContain("AI 操作…");

    const fenced = setup("```\n/");
    expect(slashCompletions(new CompletionContext(fenced, fenced.doc.length, false), slash)).toBeNull();
    view.destroy();
  });
});

describe("斜杠菜单文案", () => {
  /** 在一份语言资源里按点分路径取文案（取不到为 `undefined`）。 */
  function lookup(bundle: unknown, key: string): unknown {
    return key.split(".").reduce<unknown>((node, part) => (node && typeof node === "object" ? (node as Record<string, unknown>)[part] : undefined), bundle);
  }

  it("每一项的文案键都在包自带的两种语言里（网页与桌面同一份）", () => {
    for (const cmd of SLASH_COMMANDS) {
      for (const [locale, bundle] of Object.entries(EDITOR_CORE_LOCALES)) {
        expect(typeof lookup(bundle, cmd.labelKey), `${locale} ${cmd.id}`).toBe("string");
      }
    }
  });
});
