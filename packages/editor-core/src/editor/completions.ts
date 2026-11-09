// Wiki-link + tag 补全（PRD-06-031 P1）。
// 输入 `[[` 触发笔记名补全；输入 `#` 触发标签补全。
// 笔记候选与标签次数都由宿主给（`EditorHost.wikiLinkCandidates` / `tagCounts`，桌面问 Rust：含同名消歧，
// 标签次数与标签浏览器同一后端事实）；这里只管何时触发、怎么排进补全列表。

import { autocompletion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import type { EditorView } from "@codemirror/view";
import { isInsideFencedCode } from "./code-fence-editing";
import { matchSlashCommands, SLASH_TRIGGER, type SlashCommand } from "./slash-commands";
import type { EditorHost } from "./editor-host";

/** 斜杠菜单：条目文案（`SlashCommand.labelKey`）的取法与执行都归宿主。 */
export interface SlashMenu {
  label: (key: string) => string;
  run: (view: EditorView, command: SlashCommand) => void;
  /** 此刻不提供的条目（副库窗口没有 AI，PRD-06-050）。 */
  hidden?: (command: SlashCommand) => boolean;
}

interface CompletionsContext {
  /** 当前笔记（同名消歧的锚点）。 */
  notePath: () => string | null;
  /** 斜杠菜单；缺省不开。 */
  slash?: SlashMenu;
}

interface NoteOption { label: string; type: "class"; detail: string; apply: string }
interface TagOption { label: string; type: "variable"; detail: string; apply: string }

/**
 * 候选、排序与同名笔记写名字还是写路径都归宿主（`EditorHost.wikiLinkCandidates`）。按 frontmatter
 * 别名命中的候选（C2）列出别名、旁注指向的真名，选中写成 `[[真名|别名]]`。
 */
export async function fetchNoteCompletions(
  host: EditorHost,
  ctx: Pick<CompletionsContext, "notePath">,
  query: string,
): Promise<NoteOption[]> {
  const notePath = ctx.notePath();
  if (!notePath) return [];
  const candidates = await host.wikiLinkCandidates(notePath, query);
  return candidates.map((c) =>
    c.alias === null
      ? { label: c.title, type: "class" as const, detail: c.folder, apply: c.insert }
      : {
          label: c.alias,
          type: "class" as const,
          detail: [`→ ${c.title}`, c.folder].filter(Boolean).join(" · "),
          apply: `${c.insert}|${c.alias}`,
        },
  );
}

async function fetchTagCompletions(host: EditorHost, prefix: string): Promise<TagOption[]> {
  const tags = await host.tagCounts();
  if (tags === null) return [];
  // 补全候选**不分组**：`#` 后面要打出来的是任何一条已有标签，用户标签与 AI 标签在
  // 这里是同一池（PRD-16-043 的分组只作用于「知识库 → 标签」那棵树的呈现）。
  return tags
    .filter((t) => t.tag.toLowerCase().includes(prefix.toLowerCase()))
    .slice(0, 50)
    .map((t) => ({
      label: t.tag,
      type: "variable" as const,
      detail: `${t.noteCount}`,
      apply: t.tag,
    }));
}

/** 斜杠菜单的候选：选中即删掉 `/查询词` 再执行动作（代码块里不触发——那里的 `/` 是代码）。 */
export function slashCompletions(cmCtx: CompletionContext, slash: NonNullable<CompletionsContext["slash"]>): CompletionResult | null {
  const line = cmCtx.state.doc.lineAt(cmCtx.pos);
  const match = SLASH_TRIGGER.exec(line.text.slice(0, cmCtx.pos - line.from));
  if (!match || isInsideFencedCode(cmCtx.state, cmCtx.pos)) return null;
  const from = cmCtx.pos - match[1].length - match[2].length;
  const label = (cmd: SlashCommand) => slash.label(cmd.labelKey);
  const commands = matchSlashCommands(match[2], label).filter((cmd) => !slash.hidden?.(cmd));
  if (commands.length === 0) return null;
  return {
    from,
    filter: false,
    options: commands.map((cmd) => ({
      label: label(cmd),
      type: "keyword",
      apply: (view: EditorView, _completion: unknown, applyFrom: number, applyTo: number) => {
        view.dispatch({ changes: { from: applyFrom, to: applyTo, insert: "" }, selection: { anchor: applyFrom } });
        slash.run(view, cmd);
      },
    })),
  };
}

export function completionsExtension(host: EditorHost, ctx: CompletionsContext) {
  return autocompletion({
    // CM 按 `type` 画的类型图标（keyword → 🔑）对散文补全没有意义，斜杠菜单里只剩噪音。
    icons: false,
    override: [
      async (cmCtx: CompletionContext): Promise<CompletionResult | null> => {
        const slash = ctx.slash ? slashCompletions(cmCtx, ctx.slash) : null;
        if (slash) return slash;
        const wikilinkFrom = cmCtx.matchBefore(/\[\[[^\]|]*/u);
        if (wikilinkFrom && wikilinkFrom.text.startsWith("[[")) {
          // 替换 `[[` 之后打过的全部文字：选中同名笔记时写进去的是路径，不能只换最后一个词。
          const query = wikilinkFrom.text.slice(2);
          const options = await fetchNoteCompletions(host, ctx, query);
          if (options.length === 0) return null;
          return { from: wikilinkFrom.from + 2, options, filter: false };
        }
        const tagFrom = cmCtx.matchBefore(/#[\w-]*/u);
        if (tagFrom && tagFrom.text.startsWith("#")) {
          const tag = tagFrom.text.slice(1);
          const word = cmCtx.matchBefore(/[\w-]+/u);
          const from = word ? word.from : cmCtx.pos - tag.length;
          const options = await fetchTagCompletions(host, tag);
          if (options.length === 0) return null;
          return { from, options, validFor: /^[\w-]*$/u };
        }
        return null;
      },
    ],
  });
}
