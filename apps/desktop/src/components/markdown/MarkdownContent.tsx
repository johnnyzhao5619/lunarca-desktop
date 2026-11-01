/**
 * 共享 Markdown 渲染：`lib/markdown.ts` 节点树 → React 元素（无 innerHTML）。
 * 消费方：对话消息（`ChatMarkdown`）与笔记阅读模式（`ReadingView`）。
 *
 * 覆盖标题/列表/任务/表格/引用/围栏代码（带语言标签 + 一键复制）/行内样式/链接。
 * 外链仅 http(s) 且经 Rust `open_external_url` 系统浏览器打开（协议白名单在 Rust，
 * 前端再做一次前置校验，纵深防御）。
 *
 * 定制点（阅读模式所需，缺省行为与对话一致）：
 * - `renderText`：行内纯文本增强（wiki-link / #tag 可点击化）；
 * - `renderImage`：图片渲染策略（Vault 相对图片就地渲染 vs 外链呈现）。
 * - `openVaultLink`：**库内**相对链接（`[标题](../Notes/甲.md)`）点开另一篇笔记。
 *   缺省不给即照旧退成纯文本——对话消息里没有"打开另一篇"这回事。
 */

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { CodeCopyButton } from "./CodeCopyButton";
import { MathView } from "./MathView";
import { MermaidView } from "./MermaidView";
import {
  ensureFenceLanguage,
  highlightCodeSpans,
  isFenceLanguageReady,
} from "../../lib/code-highlight";
import {
  isSafeExternalUrl,
  parseChatMarkdown,
  type MdBlock,
  type MdInline,
  type MdListItem,
  type TableAlign,
} from "../../lib/markdown";

export interface MarkdownRenderOptions {
  /** 行内纯文本定制渲染；返回 null 表示走默认（原样文本）。 */
  renderText?: (text: string, key: number) => ReactNode;
  /** 图片定制渲染；缺省按外链/纯文本呈现（默认离线纪律，不远程加载）。 */
  renderImage?: (src: string, alt: string, key: number, width: number | null) => ReactNode;
  /**
   * 库内相对链接的打开通道；缺省不给 = 这些链接照旧是纯文本。
   *
   * **这不是可有可无的装饰**：全部自动生成的溯源链接都是这个形态（PRD-06-030），
   * 蒸馏 / 汇总 / 主题页初稿的「来源」区逐条如此。缺了它，用户点不动自己刚生成的那篇
   * 产物的来源（真机 2026-09-09）。`href` 原样交出去，解析归 Rust；`occurrence` 是同一
   * href 在全文里的第几次出现，供知识产物按声明出处定位（判定同样在 Rust）。
   */
  openVaultLink?: (href: string, occurrence: number) => void;
  /**
   * Obsidian 嵌入 `![[…]]`（对标方案 D5）的渲染；`block` = 整段只有这一条（可以画成一块）。
   * 缺省按原文显示——对话、摘录等没有「库」的场合不去解析。
   */
  renderEmbed?: (embed: { target: string; display: string | null }, key: number, block: boolean) => ReactNode;
}

/** 嵌入的原文写法（没有渲染通道或指不到时照原样显示）。 */
export function embedSource(embed: { target: string; display: string | null }): string {
  return `![[${embed.target}${embed.display === null ? "" : `|${embed.display}`}]]`;
}

/**
 * 脚注引用：上标编号，悬停显示脚注内容，点一下滚到同一篇里的定义。配不上定义的引用照原样
 * 显示 `[^id]`——看起来像脚注、点下去什么也没有，比原样更糟。
 */
function FootnoteRef({ id, note }: { id: string; note: string | null }) {
  if (note === null) return <>{`[^${id}]`}</>;
  return (
    <sup className="luna-footnote-ref">
      <button
        type="button"
        title={note}
        onClick={(event) => {
          const root = event.currentTarget.closest(".luna-markdown");
          const target = Array.from(root?.querySelectorAll<HTMLElement>("[data-footnote-id]") ?? []).find(
            (element) => element.dataset.footnoteId === id,
          );
          target?.scrollIntoView({ block: "center", behavior: "smooth" });
        }}
      >
        {id}
      </button>
    </sup>
  );
}

function renderInlines(inlines: MdInline[], options: MarkdownRenderOptions): ReactNode[] {
  return inlines.map((node, index) => {
    switch (node.kind) {
      case "text": {
        const custom = options.renderText?.(node.text, index);
        return custom ?? node.text;
      }
      case "strong":
        return <strong key={index}>{renderInlines(node.children, options)}</strong>;
      case "em":
        return <em key={index}>{renderInlines(node.children, options)}</em>;
      case "del":
        return <del key={index}>{renderInlines(node.children, options)}</del>;
      case "code":
        return <code key={index}>{node.text}</code>;
      case "break":
        return <br key={index} />;
      case "math":
        return <MathView key={index} tex={node.tex} display={false} />;
      case "footnote":
        return <FootnoteRef key={index} id={node.id} note={node.note} />;
      case "embed":
        return options.renderEmbed?.(node, index, false) ?? <span key={index}>{embedSource(node)}</span>;
      case "image": {
        const custom = options.renderImage?.(node.src, node.alt, index, node.width);
        if (custom !== undefined) {
          return custom;
        }
        // 缺省：图片按链接语义呈现（不发起远程加载）；非 http(s) 目标退纯文本。
        return renderExternalLink(
          node.src,
          [{ kind: "text", text: node.alt || node.src }],
          index,
          options,
        );
      }
      case "link":
        return renderLink(node.href, node.occurrence ?? 0, node.children, index, options);
    }
  });
}

/**
 * 一条链接：外链交系统浏览器，库内相对链接交调用方（缺省退纯文本）。
 *
 * 顺序不能颠倒——`isSafeExternalUrl` 先答"这是不是站外的"，只有它说不是，才轮到
 * "那它是不是库里的另一篇"。
 */
function renderLink(
  href: string,
  occurrence: number,
  children: MdInline[],
  key: number,
  options: MarkdownRenderOptions,
): ReactNode {
  if (isSafeExternalUrl(href) || options.openVaultLink === undefined) {
    return renderExternalLink(href, children, key, options);
  }
  const open = options.openVaultLink;
  return (
    <button
      key={key}
      type="button"
      className="luna-markdown__link"
      title={href}
      data-link-kind="md"
      data-link-target={href}
      onClick={() => open(href, occurrence)}
    >
      {renderInlines(children, options)}
    </button>
  );
}

function renderExternalLink(
  href: string,
  children: MdInline[],
  key: number,
  options: MarkdownRenderOptions,
): ReactNode {
  if (!isSafeExternalUrl(href)) {
    // 非 http(s) 目标不做成可点击链接，按纯文本呈现（防 scheme 注入）。
    return <span key={key}>{renderInlines(children, options)}</span>;
  }
  return (
    <a
      key={key}
      href={href}
      title={href}
      onClick={(event) => {
        event.preventDefault();
        void commands.openExternalUrl(href);
      }}
    >
      {renderInlines(children, options)}
    </a>
  );
}

/**
 * 代码正文 → 着色后的 React 元素树（**零 `dangerouslySetInnerHTML`**）。
 *
 * token 区间之间的空隙原样输出为文本节点，故无论解析器覆盖到哪，正文一个字符都不会丢。
 */
function renderCode(code: string, lang: string): ReactNode {
  const spans = highlightCodeSpans(code, lang);
  if (!spans || spans.length === 0) {
    // 语言未知 / 语法包尚未就绪：诚实降级为纯文本（与编辑面同一口径）。
    return code;
  }
  const out: ReactNode[] = [];
  let pos = 0;
  for (const span of spans) {
    if (span.from > pos) {
      out.push(code.slice(pos, span.from));
    }
    out.push(
      <span key={span.from} className={span.className}>
        {code.slice(span.from, span.to)}
      </span>,
    );
    pos = span.to;
  }
  if (pos < code.length) {
    out.push(code.slice(pos));
  }
  return out;
}

function CodeBlock({ lang, code }: { lang: string; code: string }) {
  const { t } = useTranslation("common");
  // 语法包按需惰性加载（与编辑面同一个 language-data 目录）。同一种语言在整个会话里
  // 只加载一次，故初值同步问一次「就绪没有」——已加载过的语言首帧就是彩色的，不闪。
  const [ready, setReady] = useState(() => isFenceLanguageReady(lang));

  useEffect(() => {
    let active = true;
    setReady(isFenceLanguageReady(lang));
    void ensureFenceLanguage(lang).then((ok) => {
      if (active) {
        setReady(ok);
      }
    });
    return () => {
      active = false;
    };
  }, [lang]);

  const body = useMemo(() => (ready ? renderCode(code, lang) : code), [code, lang, ready]);

  return (
    <div className="luna-markdown-codeblock">
      <div className="luna-markdown-codeblock__header">
        <span>{lang || t("markdown.plainText")}</span>
        <CodeCopyButton code={code} />
      </div>
      <pre>
        <code>{body}</code>
      </pre>
    </div>
  );
}

/** 未声明对齐的列不写属性（让 CSS 的默认 `text-align: start` 生效，RTL 零成本）。 */
function alignAttr(align: TableAlign | undefined): string | undefined {
  return align ?? undefined;
}

function renderListItem(
  item: MdListItem,
  index: number,
  options: MarkdownRenderOptions,
): ReactNode {
  return (
    <li key={index} data-task={item.task ?? undefined}>
      {item.task ? (
        <input type="checkbox" checked={item.task === "done"} readOnly tabIndex={-1} />
      ) : null}
      {renderBlocks(item.blocks, options)}
    </li>
  );
}

function renderBlocks(blocks: MdBlock[], options: MarkdownRenderOptions): ReactNode[] {
  return blocks.map((block, index) => {
    switch (block.kind) {
      case "heading": {
        const Tag = `h${block.level}` as const;
        return <Tag key={index}>{renderInlines(block.children, options)}</Tag>;
      }
      case "paragraph":
        return <p key={index}>{renderInlines(block.children, options)}</p>;
      case "embed":
        return options.renderEmbed?.(block, index, true) ?? <p key={index}>{embedSource(block)}</p>;
      case "code_block":
        return block.lang.trim().toLowerCase() === "mermaid" ? (
          <MermaidView key={index} code={block.code} />
        ) : (
          <CodeBlock key={index} lang={block.lang} code={block.code} />
        );
      case "math":
        return <MathView key={index} tex={block.tex} display />;
      case "footnote":
        return (
          <div key={index} className="luna-footnote" data-footnote-id={block.id}>
            <span className="luna-footnote__label">{block.id}</span>
            <span>{renderInlines(block.children, options)}</span>
          </div>
        );
      case "blockquote":
        return <blockquote key={index}>{renderBlocks(block.blocks, options)}</blockquote>;
      case "callout":
        // 折叠用原生 `<details>`：`> [!note]-` 的折叠语义天然对应它，无需自建开合状态，
        // 键盘可达与"展开后可搜索"也一并由浏览器负责。
        return (
          <details
            key={index}
            className="luna-md-callout"
            data-callout={block.tone}
            open={!block.defaultFolded}
          >
            <summary className="luna-md-callout__head">
              <span className="luna-md-callout__label">{block.label}</span>
              {block.title.length > 0 && (
                <span className="luna-md-callout__title">
                  {renderInlines(block.title, options)}
                </span>
              )}
            </summary>
            <div className="luna-md-callout__body">{renderBlocks(block.blocks, options)}</div>
          </details>
        );
      case "list":
        return block.ordered ? (
          <ol key={index}>{block.items.map((item, i) => renderListItem(item, i, options))}</ol>
        ) : (
          <ul key={index}>{block.items.map((item, i) => renderListItem(item, i, options))}</ul>
        );
      case "table":
        // 逐列对齐用 `data-align` 表达，与编辑面的表格 widget 同一套属性值与同一份
        // CSS 规则（`:-:` / `-:`）——此前这里根本不读 align，同一张表切到阅读面全左对齐。
        return (
          <div key={index} className="luna-markdown__table-wrap">
            <table>
              <thead>
                <tr>
                  {block.header.map((cell, cellIndex) => (
                    <th key={cellIndex} data-align={alignAttr(block.align[cellIndex])}>
                      {renderInlines(cell, options)}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {block.rows.map((row, rowIndex) => (
                  <tr key={rowIndex}>
                    {row.map((cell, cellIndex) => (
                      <td key={cellIndex} data-align={alignAttr(block.align[cellIndex])}>
                        {renderInlines(cell, options)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
      case "hr":
        return <hr key={index} />;
    }
  });
}

export interface MarkdownContentProps extends MarkdownRenderOptions {
  content: string;
  /** 流式进行中：末尾追加闪烁打字光标（对话场景，PRD-03 验收）。 */
  streaming?: boolean;
}

export function MarkdownContent({
  content,
  streaming = false,
  renderText,
  renderImage,
  openVaultLink,
  renderEmbed,
}: MarkdownContentProps) {
  const blocks = useMemo(() => parseChatMarkdown(content), [content]);
  const options = useMemo(
    () => ({ renderText, renderImage, openVaultLink, renderEmbed }),
    [renderText, renderImage, openVaultLink, renderEmbed],
  );
  return (
    <div className="luna-markdown">
      {renderBlocks(blocks, options)}
      {streaming ? <span className="luna-chat-cursor" /> : null}
    </div>
  );
}
