// 编辑器的组装（PRD-21-020：与桌面同一套编辑器；TDD-17 §2）。桌面与网页都从这里拿到同一份扩展列表——
// 同一种 Markdown 方言、同一套括号配对、同一串键位与它们的先后、同一组实时预览渲染件——宿主只往几个具名的
// 插槽里追加自己专属的扩展（桌面的 AI 会话、时间码装订线、朗读高亮、附件粘贴……）。
//
// **键位的先后就是行为**：同一优先级里先装的先消费按键（围栏的回车先于列表续写、表格里的 Tab 先于缩进、
// 一切自定义键先于 CM 默认键表）。插槽的位置因此是契约的一部分：桌面 AI 会话的 Esc 必须先于围栏 / 表格 /
// 列表，所以它有自己的插槽，而不是追加在末尾。
//
// 文案是包自带的（`editor-core` 命名空间的 `editor.*`），宿主只交它的 i18next `t`：两个宿主不各存一份
// 「查找」「折叠」「元数据 (YAML)」。

import { closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, historyKeymap } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { search, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, drawSelection, highlightActiveLine, keymap, tooltips } from "@codemirror/view";
import { codeLanguageForFence } from "../highlight/code-highlight";
import { lunaSyntaxHighlighting } from "../highlight/markdown-highlight";
import { EDITOR_MARKDOWN_EXTENSIONS } from "../markdown/markdown-dialect";
import { blockMoveKeymap } from "./block-move";
import { blockSpacingExtension } from "./block-spacing";
import { codeBlockLinesExtension } from "./code-block-lines";
import { codeFenceEditingKeymap } from "./code-fence-editing";
import { completionsExtension, type SlashMenu } from "./completions";
import { editorProductivityExtensions, lunaScaffoldHidingExtension, markdownCloseBracketsData } from "./editor-extensions";
import type { EditorHost } from "./editor-host";
import { frontmatterFoldingExtension, frontmatterLinesExtension } from "./frontmatter";
import { headingFoldExtension } from "./heading-fold";
import { listEditingKeymap } from "./list-editing";
import { listIndentExtension } from "./list-indent";
import { livePreviewExtension } from "./live-preview";
import { inlineTagExtension, taskCheckboxExtension } from "./markdown-decorations";
import { formatKeyBindings } from "./markdown-format";
import { mdLinkClickHandler } from "./md-link";
import { multiCursorExtension } from "./multi-cursor";
import { unresolvedLinkExtension } from "./reference-links";
import { richPreviewExtension } from "./rich-preview";
import { selectionLayerExtension } from "./selection-layer";
import { tableEditingKeymap } from "./table-edit";
import { tablePickerExtension } from "./table-picker";
import { tablePreviewExtension, type TableLabels } from "./table-preview";
import { wikiLinkClickHandler, wikiLinkExtension } from "./wiki-link";

/** 编辑器文案的取法：宿主 i18next 实例的 `t`（本包的键一律带 `editor-core:` 前缀）。 */
export type EditorTranslate = (key: string, options?: Record<string, unknown>) => string;

/** 查找面板与跳行的文案（CM 的 phrases）：随界面语言热重配，宿主把它装进自己的文案隔间。 */
export function editorPhrases(t: EditorTranslate): Extension {
  return EditorState.phrases.of({
    Find: t("editor-core:editor.search.find"),
    Replace: t("editor-core:editor.search.replace"),
    next: t("editor-core:editor.search.next"),
    previous: t("editor-core:editor.search.previous"),
    all: t("editor-core:editor.search.all"),
    "match case": t("editor-core:editor.search.matchCase"),
    regexp: t("editor-core:editor.search.regexp"),
    "by word": t("editor-core:editor.search.byWord"),
    replace: t("editor-core:editor.search.replaceOne"),
    "replace all": t("editor-core:editor.search.replaceAll"),
    close: t("editor-core:editor.search.close"),
    "current match": t("editor-core:editor.search.currentMatch"),
    "on line": t("editor-core:editor.search.onLine"),
    "Go to line": t("editor-core:editor.search.goToLine"),
    go: t("editor-core:editor.search.go"),
    "replaced $ matches": t("editor-core:editor.search.replacedMatches"),
    "replaced match on line $": t("editor-core:editor.search.replacedOnLine"),
  });
}

/** 实时预览渲染件要回到宿主的动作。 */
export interface LivePreviewOptions {
  t: EditorTranslate;
  /** 当前笔记的库内路径（相对链接、图片与嵌入的解析锚点）；还没落盘为 `null`。 */
  notePath: () => string | null;
  /** 打开库内的一个文件（相对链接、嵌入的「打开」）。 */
  onOpenPath: (path: string) => void;
  /** 点开一条 `[[wiki]]`（`target` 是 `|` 之前的原文）。 */
  onActivateWikiLink: (target: string) => void;
  /** 点一个行内 `#标签`。 */
  onOpenTag: (tag: string) => void;
  /** 属性面板里认不出的写法：「在源码中编辑」——切到源码模式并把光标放到第 `line` 行（1 起）。 */
  onEditSource: (view: EditorView, line: number) => void;
  /** 表格的宿主动作（行列把手菜单、复制多格、粘贴进单元格）；不给就没有这几样。 */
  table?: Pick<TableLabels, "openMenu" | "copyRange" | "pasteCell">;
}

/**
 * 实时预览装饰组（就地渲染 + 可点交互）：只在实时预览模式装进宿主的预览隔间，源码模式置空（PRD-06-010/011/017）。
 * 文案在构造时取值，所以界面语言变了宿主要重配这一组。
 */
export function livePreviewExtensions(host: EditorHost, options: LivePreviewOptions): Extension {
  const { t, notePath } = options;
  const wikiCtx = { notePath, onActivate: options.onActivateWikiLink };
  return [
    livePreviewExtension(host, notePath, {
      view: t("editor-core:editor.image.view"),
      resize: t("editor-core:editor.image.resize"),
      lightbox: {
        dialog: t("editor-core:reading.lightbox.dialog"),
        close: t("editor-core:reading.lightbox.close"),
        previous: t("editor-core:reading.lightbox.previous"),
        next: t("editor-core:reading.lightbox.next"),
      },
    }),
    // 表格块就地渲染（选区触及即还原源码；PRD-06-011 P1）。
    tablePreviewExtension({
      cell: (row, column) => t("editor-core:editor.table.cell", { row, column }),
      rowHandle: (row) => t("editor-core:editor.table.rowHandle", { row }),
      columnHandle: (column) => t("editor-core:editor.table.columnHandle", { column }),
      appendRow: t("editor-core:editor.table.appendRow"),
      appendColumn: t("editor-core:editor.table.appendColumn"),
      ...options.table,
    }),
    // 公式（`$…$` / `$$…$$`）与 Mermaid 图表：光标不在里面时显示渲染结果（对标方案 D1 / D2）。
    // 嵌入 `![[…]]`（D5）：独占一行的在光标外换成渲染结果，打开与跳转同点击链接一个动作。
    richPreviewExtension(host, {
      embed: { notePath, onOpenPath: options.onOpenPath, onActivateWikiLink: options.onActivateWikiLink },
    }),
    taskCheckboxExtension(),
    inlineTagExtension(options.onOpenTag),
    wikiLinkExtension(host, wikiCtx),
    wikiLinkClickHandler(wikiCtx),
    // 标准 md 链接（自动生成的「来源」区逐条如此）：与阅读面同一条解析。
    mdLinkClickHandler(host, { notePath, onOpenPath: options.onOpenPath }),
    // 机器脚手架注释（转写块哨兵、对话笔记的出处标记）在预览面让位：它们是 Rust 的定界符，
    // 不是任何人写下的内容。与 frontmatter 折叠同处本组 = 同样只作用于实时预览。
    lunaScaffoldHidingExtension(),
    frontmatterFoldingExtension(
      host,
      {
        label: (count) =>
          count > 0 ? t("editor-core:editor.frontmatter.labelCount", { count }) : t("editor-core:editor.frontmatter.label"),
        expandTitle: t("editor-core:editor.frontmatter.expandTitle"),
        collapseTitle: t("editor-core:editor.frontmatter.collapseTitle"),
        expandAction: t("editor-core:editor.frontmatter.expandAction"),
        collapseAction: t("editor-core:editor.frontmatter.collapseAction"),
      },
      // 属性面板里认不出的写法：切到源码模式、光标落到那一行（ADR-139 ①）。
      { onEditSource: options.onEditSource },
    ),
  ];
}

/**
 * 宿主专属扩展的插槽。位置即先后（见文件抬头）；不需要的插槽不给。
 */
export interface MarkdownEditorSlots {
  /** 文案隔间之后：宿主按设置热切换的隔间（桌面：打字机、拼写检查）。 */
  afterLocale?: Extension;
  /** 编辑键位链的最前（围栏 / 表格 / 列表键位之前）：宿主要先于它们消费的键（桌面：AI 会话的 Esc / ⌘↩）。 */
  beforeEditingKeymaps?: Extension;
  /** 实时预览隔间之后：宿主的行装饰与装订线（桌面：时间码、朗读高亮）。 */
  afterPresentation?: Extension;
  /** 补全之后：宿主的粘贴与拖入处理（桌面：附件落盘、远程图片本地化）。 */
  afterCompletions?: Extension;
  /** 基础键位表之前：宿主的 DOM 事件与更新监听（失焦保存、脏判定）。 */
  beforeBaseKeymap?: Extension;
}

export interface MarkdownEditorOptions {
  t: EditorTranslate;
  /** 当前笔记（补全的同名消歧锚点）。 */
  notePath: () => string | null;
  /**
   * 宿主持有的四个隔间，各装好初值交进来（换篇重建撤销历史、装载闸、换界面语言、换编辑模式时由宿主
   * 各自 `reconfigure`）：撤销历史（`history()`）、只读闸（`EditorState.readOnly`）、文案
   * （`editorPhrases` + 宿主的占位提示）、实时预览（`livePreviewExtensions`，源码模式为空）。
   */
  history: Extension;
  readOnly: Extension;
  locale: Extension;
  presentation: Extension;
  /** 斜杠菜单：文案与执行归宿主（条目文案与右键菜单、工具栏同一批键）；不给就不开斜杠菜单。 */
  slash?: SlashMenu;
  slots?: MarkdownEditorSlots;
}

/** 一个 Markdown 编辑器的全部扩展（桌面与网页同一份），宿主专属的经 `slots` 插进来。 */
export function markdownEditorExtensions(host: EditorHost, options: MarkdownEditorOptions): Extension[] {
  const { t, slots = {} } = options;
  return [
    options.history,
    options.readOnly,
    // GFM 基线方言（表格/任务列表/删除线/自动链接，PRD-06-016）；
    // 围栏代码块按 CodeInfo 惰性加载语言解析器（Typora 对标，PRD-06-011）。
    // `addKeymap: false`：语言包自带的 Enter / Backspace 键位挂在 Prec.high，压过
    // 本包的全部绑定，且它的退格会把列表标记换成等宽空格、把任务项变成缩进代码块
    // （真机 2026-09-04）。列表 / 引用块的键位由 list-editing.ts + editor-extensions.ts
    // 单一来源承担。
    markdown({
      base: markdownLanguage,
      // 与阅读视图、大纲同一方言（`markdown-dialect`：CJK 友好强调）。
      extensions: EDITOR_MARKDOWN_EXTENSIONS,
      codeLanguages: codeLanguageForFence,
      addKeymap: false,
    }),
    // 括号/引号智能配对（Typora 对标；退格删对、选区包裹）。
    closeBrackets(),
    // markdown 语言级配对集（补反引号；引号在字词后不配对为 CM 既定防误配策略）。
    markdownCloseBracketsData(),
    search({ top: false }),
    // 查找文案与正文提示随界面语言热重配，不重建正文或撤销栈。
    options.locale,
    slots.afterLocale ?? [],
    highlightActiveLine(),
    // CM 自绘光标与选区（`drawSelection`）：原生光标贴着块级部件（折叠的 frontmatter
    // 胶囊）时，WebKit 会画一根与部件同高的粗光标（真机 2026-09-02，颜色修对了形状还在）；
    // CM 按文本坐标画的光标永远是一根细线。颜色归 `@lunarca/styles/codemirror.css`。
    drawSelection(),
    // 选区矩形改由自绘层画（左界夹到正文列、换行残块不画）；CM 自带的选区层由
    // `@lunarca/styles/codemirror.css` 藏起来，理由见 selection-layer.ts 抬头。
    selectionLayerExtension(),
    // 浮层（补全、斜杠菜单、表格尺寸选择器）的可用空间按正文可见区算，而不是整个窗口：
    // 否则光标贴近底部时 CM 以为下方还放得下，浮层落到状态栏后面被裁掉（真机 2026-10-04
    // 两条 `[[` 补全只露出一半）。可见区放不下就翻到光标上方。
    tooltips({ tooltipSpace: (view) => view.scrollDOM.getBoundingClientRect() }),
    // 多光标（B5）：⌥ 点击加光标、⌥ 拖动列选；只看主光标的自定义按键在多光标下放行。
    multiCursorExtension(),
    slots.beforeEditingKeymaps ?? [],
    // 围栏的自动闭合与空行退出；置于列表续写/默认 Enter 之前。
    codeFenceEditingKeymap(),
    // 表格内 Tab/Shift-Tab 跳单元格（先于缩进 keymap 消费；表外落回缩进）。
    tableEditingKeymap(),
    tablePickerExtension({
      title: t("editor-core:editor.tablePicker.title"),
      size: (columns, rows) => t("editor-core:editor.tablePicker.size", { columns, rows }),
    }),
    // 列表编辑手感：项首退格 / Shift+Enter 软换行 / 按列表结构的 Tab、Shift-Tab；
    // 表外、列表外一律放行给下面的通用缩进与默认退格。
    listEditingKeymap(),
    editorProductivityExtensions(host),
    // ⌥↑↓ 先按块移动（列表项连子项、表格、引用、代码块围栏），不归它管再逐行移动。
    blockMoveKeymap(),
    // 标题按章节、列表项连子项折叠（B3）：悬停出三角，⌘⌥[ / ⌘⌥]。
    headingFoldExtension({ fold: t("editor-core:editor.fold.fold"), unfold: t("editor-core:editor.fold.unfold") }),
    options.presentation,
    slots.afterPresentation ?? [],
    completionsExtension(host, { notePath: options.notePath, slash: options.slash }),
    slots.afterCompletions ?? [],
    lunaSyntaxHighlighting(),
    // 代码块整块底色容器（Typora 对标；实时预览 / 源码两模式共用）。
    codeBlockLinesExtension(host),
    // 列表行悬挂缩进（同为两模式共用：源码里标记可见，宽度同样成立）。
    listIndentExtension(),
    // 文首 frontmatter 按 YAML 原文显示（源码模式；实时预览里整段是胶囊 / 属性面板）。
    frontmatterLinesExtension(),
    // 块间距（同为两模式共用：间距不替换任何标记，源码模式照样是长文）。
    blockSpacingExtension(),
    // 未解析引用式链接去链接色（同为两模式共用：`[名字]` 是不是链接取决于文档里
    // 有没有对应定义，与用哪个视图看无关）。
    unresolvedLinkExtension(),
    EditorView.lineWrapping,
    slots.beforeBaseKeymap ?? [],
    // 键位一律从命令注册表派生（TDD-07 §4.2）：右键菜单与格式工具栏的提示取同一张表。
    keymap.of([
      // 加粗 / 斜体 / 链接与 Typora 风格段落键 ⌘0–⌘6（PRD-06-014）。
      ...formatKeyBindings(() => t("editor-core:editor.linkPlaceholder")),
      ...closeBracketsKeymap,
      ...searchKeymap,
      ...defaultKeymap,
      ...historyKeymap,
    ]),
  ];
}
