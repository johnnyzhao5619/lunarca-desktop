/**
 * 阅读模式视图（PRD-06-017，Obsidian Reading view 对标）：只读渲染当前笔记。
 *
 * - 正文经共享 `MarkdownContent` 渲染（与对话消息同一解析基线，DRY）；
 * - frontmatter 呈现为「属性」块（只读，不改写事实源）；
 * - wiki-link / #tag 保持可点击（与编辑器装饰同一语法事实源）；
 * - Vault 相对路径图片经 asset 协议就地渲染（PRD-06-013）；http(s) 图片按外链
 *   呈现不远程加载（默认离线纪律）。
 */

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { embedSource, MarkdownContent } from "../../components/markdown/MarkdownContent";
import { useAssetGeneration } from "../media/asset-changes";
import { vaultImageAssetSrc } from "../media/media-src";
import { embedTargetsOf, isSafeExternalUrl, splitImageWidth } from "../../lib/markdown";
import { commands, type EmbedHitDto } from "../../ipc/bindings";
import { openLinkLocation } from "./open-link-location";
import { frontmatterEntries, splitFrontmatter, splitReadingInline } from "./reading-view";
import { WIKI_LINK_RE } from "./wiki-link";
import { openImageLightbox } from "../../lib/image-lightbox";
import { prepareRichContent } from "../../lib/rich-content";
import { openWikiLink, resolveWikiTargets } from "./wiki-link-open";

const NO_TARGETS: ReadonlySet<string> = new Set();

/**
 * 正文里指不到笔记的 wiki-link 目标（按 Rust 的回答，与编辑面同一份解析）。没问到之前
 * 一律按已解析呈现——闪一下「未创建」比晚一拍着色更误导。
 */
function useUnresolvedWikiTargets(notePath: string | null, body: string): ReadonlySet<string> {
  const [unresolved, setUnresolved] = useState<ReadonlySet<string>>(NO_TARGETS);
  // 先折成一个字符串：正文改了但链接没变时，不必重问。
  const key = useMemo(() => {
    const found = new Set<string>();
    WIKI_LINK_RE.lastIndex = 0;
    let match: RegExpExecArray | null;
    while ((match = WIKI_LINK_RE.exec(body)) !== null) {
      const target = match[1].trim();
      if (target) found.add(target);
    }
    return [...found].join("\n");
  }, [body]);

  useEffect(() => {
    let active = true;
    const targets = key ? key.split("\n") : [];
    if (!notePath || targets.length === 0) {
      setUnresolved(NO_TARGETS);
      return;
    }
    void resolveWikiTargets(notePath, targets).then((answer) => {
      if (active && answer) {
        setUnresolved(new Set(targets.filter((_, i) => answer[i] === null)));
      }
    });
    return () => {
      active = false;
    };
  }, [notePath, key]);

  return unresolved;
}

/** 嵌入里再有嵌入时只显示成链接：不无限展开，也不会互相嵌入成环。 */
const MAX_EMBED_DEPTH = 1;

/**
 * 正文里每条嵌入 `![[…]]` 指向什么（Rust 解析，对标方案 D5）。导出时由调用方预先解析好传进来
 * （静态渲染等不到异步结果）；嵌入里的嵌入不解析。没问到之前一律当「还不知道」，按链接显示。
 */
function useEmbedHits(
  notePath: string | null,
  body: string,
  preset: ReadonlyMap<string, EmbedHitDto> | undefined,
  enabled: boolean,
): ReadonlyMap<string, EmbedHitDto> {
  const [hits, setHits] = useState<ReadonlyMap<string, EmbedHitDto>>(() => new Map());
  // 库外删掉 / 放回附件后重新解析：`![[图.png]]` 指不指得到随之变化（PRD-06-012 ③）。
  const assetGeneration = useAssetGeneration();
  const key = useMemo(() => (preset || !enabled ? "" : embedTargetsOf(body).join("\n")), [body, preset, enabled]);
  useEffect(() => {
    let active = true;
    const targets = key ? key.split("\n") : [];
    if (!notePath || targets.length === 0) {
      setHits(new Map());
      return;
    }
    void commands.vaultResolveEmbeds(notePath, targets).then((result) => {
      if (active && result.status === "ok") setHits(new Map(targets.map((target, i) => [target, result.data[i]])));
    });
    return () => {
      active = false;
    };
  }, [notePath, key, assetGeneration]);
  return preset ?? hits;
}

/**
 * 一段 Markdown 的嵌入预先解析成表（导出、打印用；ADR 见对标方案 D5「导出时展开」）：被嵌入笔记里
 * 的公式与图表一并排好——静态渲染只认已经排好的结果。被嵌入笔记里的嵌入不再解析。
 */
export async function resolveEmbedsFor(notePath: string, content: string): Promise<ReadonlyMap<string, EmbedHitDto>> {
  const targets = embedTargetsOf(content);
  if (targets.length === 0) return new Map();
  const result = await commands.vaultResolveEmbeds(notePath, targets);
  if (result.status !== "ok") return new Map();
  await Promise.all(result.data.map((hit) => (hit.kind === "note" ? prepareRichContent(hit.content) : undefined)));
  return new Map(targets.map((target, i) => [target, result.data[i]]));
}

interface VaultImageProps {
  notePath: string;
  src: string;
  alt: string;
  width: number | null;
  onOpen: (image: HTMLImageElement) => void;
}

/** Vault 相对图片：经 Rust 解析真实路径 + asset 协议加载；失败退纯文本占位。 */
function VaultImage({ notePath, src, alt, width, onOpen }: VaultImageProps) {
  const [resolved, setResolved] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  // 库外删掉 / 放回这张图之后重新解析（PRD-06-012 ③）：已显示的图变成断链，断链的图重新出现。
  const assetGeneration = useAssetGeneration();

  useEffect(() => {
    let active = true;
    setResolved(null);
    setFailed(false);
    vaultImageAssetSrc(notePath, src)
      .then((url) => {
        if (active) {
          setResolved(url);
        }
      })
      .catch(() => {
        if (active) {
          setFailed(true);
        }
      });
    return () => {
      active = false;
    };
  }, [notePath, src, assetGeneration]);

  if (failed) {
    return (
      <span className="luna-reading__image-missing" title={src}>
        {alt || src}
      </span>
    );
  }
  if (!resolved) {
    return <span className="luna-reading__image-loading" aria-hidden="true" />;
  }
  return (
    <img
      className="luna-reading__image"
      src={resolved}
      alt={alt}
      width={width ?? undefined}
      loading="lazy"
      onError={() => setFailed(true)}
      onClick={(event) => onOpen(event.currentTarget)}
    />
  );
}

export interface ReadingViewProps {
  content: string;
  /** 当前笔记 Vault 相对路径（相对图片解析锚点）。 */
  notePath: string | null;
  onOpenPath?: (path: string) => void;
  /**
   * 库内链接打开前的拦截：返回 `true` = 已处理（知识产物阅读视图用它把声明出处交给
   * Rust 按定位锚打开）；`false` 或缺省走普通的「打开那一篇」。
   */
  interceptVaultLink?: (href: string, occurrence: number) => Promise<boolean>;
  onOpenTag?: (tag: string) => void;
  /**
   * 点一条 wiki-link（`target` 为 `|` 之前的原文）。编辑器传入它，与编辑面同一个动作
   * （打开 / 跳到片段 / 新建）；缺省时只打开指得到的那一篇、不新建（只读的阅读面）。
   */
  onActivateWikiLink?: (target: string) => void;
  /**
   * 静态导出形态（HTML 导出 / 打印窗口，PRD-11-011）：图片保留**笔记相对 src**（由 Rust
   * 内联或打印窗口不需要 asset 协议之外的解析），wiki-link / 标签退成不可点的文本——
   * 导出物里没有「打开另一篇」这回事。渲染基线仍是同一份 `MarkdownContent`。
   */
  staticExport?: boolean;
  /** 预先解析好的嵌入（导出 / 打印：静态渲染等不到异步结果）。缺省时自己问 Rust。 */
  embeds?: ReadonlyMap<string, EmbedHitDto>;
  /** 嵌入层级：被嵌入的笔记以 1 渲染，它里面的嵌入只显示成链接。 */
  embedDepth?: number;
}

export function ReadingView({
  content,
  notePath,
  onOpenPath,
  interceptVaultLink,
  onOpenTag,
  onActivateWikiLink,
  staticExport = false,
  embeds,
  embedDepth = 0,
}: ReadingViewProps) {
  const { t } = useTranslation("editor");
  const { frontmatter, body } = useMemo(() => splitFrontmatter(content), [content]);
  const properties = useMemo(
    () => (frontmatter ? frontmatterEntries(frontmatter) : []),
    [frontmatter],
  );

  const unresolved = useUnresolvedWikiTargets(staticExport ? null : notePath, body);
  const embedHits = useEmbedHits(notePath, body, embeds, !staticExport && embedDepth < MAX_EMBED_DEPTH);

  function activateWikiLink(target: string) {
    if (onActivateWikiLink) {
      onActivateWikiLink(target);
      return;
    }
    if (notePath && onOpenPath) {
      void openWikiLink(notePath, target, { onOpenPath, allowCreate: false, onError: () => {} });
    }
  }

  /**
   * 库内相对链接 → 打开那一篇。
   *
   * 解析（`../` 归一、越界判定、存不存在）全在 Rust：那是文件系统的事实，前端拼一遍
   * 就是第二处会漂的定义，而漂了之后的表现是把用户送到一篇不存在的笔记。指不到就
   * **什么都不做**——不猜一个"大概是这篇"。
   */
  function openVaultLink(href: string, occurrence: number) {
    if (!notePath) {
      return;
    }
    const openNote = () =>
      void commands.vaultLinkLocation(notePath, href).then((result) => {
        if (result.status === "ok" && result.data && onOpenPath) {
          openLinkLocation(result.data, onOpenPath);
        }
      });
    if (!interceptVaultLink) {
      openNote();
      return;
    }
    void interceptVaultLink(href, occurrence).then((handled) => {
      if (!handled) {
        openNote();
      }
    });
  }

  function renderText(text: string, key: number): ReactNode {
    const parts = splitReadingInline(text);
    if (parts.length === 1 && parts[0].kind === "text") {
      return null; // 走默认原样文本，避免多余包装节点。
    }
    return (
      <span key={key}>
        {parts.map((part, index) => {
          switch (part.kind) {
            case "text":
              return part.text;
            case "wiki": {
              if (staticExport) {
                return (
                  <span key={index} className="luna-wiki-link luna-reading__wiki">
                    {part.label}
                  </span>
                );
              }
              const resolved = !unresolved.has(part.target);
              return (
                <button
                  key={index}
                  type="button"
                  className={
                    resolved
                      ? "luna-wiki-link luna-reading__wiki"
                      : "luna-wiki-link luna-wiki-link--unresolved luna-reading__wiki"
                  }
                  title={part.target}
                  data-link-kind="wiki"
                  data-link-target={part.target}
                  onClick={() => activateWikiLink(part.target)}
                >
                  {part.label}
                </button>
              );
            }
            case "tag":
              if (staticExport) {
                return (
                  <span key={index} className="luna-inline-tag luna-reading__tag">
                    #{part.tag}
                  </span>
                );
              }
              return (
                <button
                  key={index}
                  type="button"
                  className="luna-inline-tag luna-reading__tag"
                  title={t("reading.openTag", { tag: part.tag })}
                  onClick={() => onOpenTag?.(part.tag)}
                >
                  #{part.tag}
                </button>
              );
          }
        })}
      </span>
    );
  }

  /** 点阅读面里的图：看大图，←/→ 在这一篇的图之间切换（对标方案 D4）。 */
  function openLightbox(image: HTMLImageElement) {
    const images = Array.from(image.closest(".luna-reading")?.querySelectorAll<HTMLImageElement>("img.luna-reading__image") ?? []);
    openImageLightbox(
      images.map((each) => ({ src: each.src, alt: each.alt })),
      images.indexOf(image),
      { dialog: t("image.lightbox"), close: t("image.close"), previous: t("image.previous"), next: t("image.next") },
    );
  }

  function renderImage(src: string, alt: string, key: number, width: number | null): ReactNode {
    if (isSafeExternalUrl(src)) {
      if (staticExport) {
        return (
          <a key={key} href={src} title={src}>
            {alt || src}
          </a>
        );
      }
      // 外链图片不远程加载：按链接语义外开（与对话一致）。
      return (
        <a
          key={key}
          href={src}
          title={src}
          onClick={(event) => {
            event.preventDefault();
            void commands.openExternalUrl(src);
          }}
        >
          {alt || src}
        </a>
      );
    }
    if (!notePath) {
      return <span key={key}>{alt || src}</span>;
    }
    if (staticExport) {
      return <img key={key} className="luna-reading__image" src={src} alt={alt} width={width ?? undefined} />;
    }
    return <VaultImage key={key} notePath={notePath} src={src} alt={alt} width={width} onOpen={openLightbox} />;
  }

  /** 嵌入照链接显示（嵌入里的嵌入、还没解析出来、行内的笔记嵌入）：点一下打开那一篇。 */
  function embedAsLink(embed: { target: string; display: string | null }, key: number): ReactNode {
    const label = embed.display ?? embed.target;
    if (staticExport) return <span key={key} className="luna-wiki-link luna-reading__wiki">{label}</span>;
    return (
      <button key={key} type="button" className="luna-wiki-link luna-reading__wiki" title={embed.target} onClick={() => activateWikiLink(embed.target)}>
        {label}
      </button>
    );
  }

  /** 嵌入 `![[…]]`（对标方案 D5）：图片就地显示、笔记成块显示内容、其它文件是可点的名字。 */
  function renderEmbed(embed: { target: string; display: string | null }, key: number, block: boolean): ReactNode {
    const hit = embedDepth < MAX_EMBED_DEPTH ? embedHits.get(embed.target) : undefined;
    if (!hit) return embedAsLink(embed, key);
    switch (hit.kind) {
      case "missing":
        return (
          <span key={key} className="luna-embed-missing" title={t("embed.missing")}>
            {embedSource(embed)}
          </span>
        );
      case "file": {
        const name = hit.path.split("/").pop() ?? hit.path;
        if (hit.image) {
          const { width } = splitImageWidth(`|${embed.display ?? ""}`);
          return renderImage(hit.link, width === null && embed.display ? embed.display : name, key, width);
        }
        const label = embed.display ?? name;
        if (staticExport || !onOpenPath) return <span key={key}>{label}</span>;
        return (
          <button key={key} type="button" className="luna-markdown__link" title={hit.path} onClick={() => onOpenPath(hit.path)}>
            {label}
          </button>
        );
      }
      case "note":
        if (!block) return embedAsLink(embed, key);
        return (
          <div key={key} className="luna-embed" data-embed-path={hit.path}>
            <div className="luna-embed__head">
              <span className="luna-embed__title" title={hit.path}>
                {embed.display ?? embed.target}
              </span>
              {staticExport || !onOpenPath ? null : (
                <button type="button" className="luna-embed__open" onClick={() => activateWikiLink(embed.target)}>
                  {t("embed.open")}
                </button>
              )}
            </div>
            <ReadingView
              content={hit.content}
              // 被嵌入的内容里的相对链接已按宿主改写（Rust `rebase_links`），故按宿主解析图片。
              notePath={notePath}
              onOpenPath={onOpenPath}
              onOpenTag={onOpenTag}
              onActivateWikiLink={onActivateWikiLink}
              staticExport={staticExport}
              embedDepth={embedDepth + 1}
            />
          </div>
        );
    }
  }

  return (
    <div className="luna-reading" data-testid="reading-view">
      {properties.length > 0 ? (
        <section className="luna-reading__props" aria-label={t("reading.properties")}>
          <h4 className="luna-reading__props-title">{t("reading.properties")}</h4>
          <dl>
            {properties.map((entry) => (
              <div key={entry.key} className="luna-reading__prop">
                <dt>{entry.key}</dt>
                <dd>{entry.value || "—"}</dd>
              </div>
            ))}
          </dl>
        </section>
      ) : null}
      <MarkdownContent
        content={body}
        renderText={renderText}
        renderImage={renderImage}
        // 库内相对链接（自动生成的「来源」区逐条如此）：解析归 Rust，这里只负责打开。
        // 导出物里没有"打开另一篇"这回事，故静态导出不给这条通道。
        openVaultLink={staticExport || !notePath || !onOpenPath ? undefined : openVaultLink}
        renderEmbed={notePath ? renderEmbed : undefined}
      />
    </div>
  );
}
