/**
 * 阅读视图（PRD-06-017、PRD-21-012；Obsidian Reading view 对标）：只读渲染一篇笔记。桌面与网页共用这一份
 * （TDD-17 §2）——宿主只注入两样端口：`host`（外链、HTML 出口、复制按钮）与 `vault`（库级能力）。
 *
 * - 正文经共享 `MarkdownContent` 渲染（与对话消息同一解析基线）；
 * - frontmatter 呈现为折叠的「属性」块（只读，不改写事实源）；
 * - wiki-link / #tag 保持可点击（与编辑器装饰同一语法事实源）；
 * - 库内相对图片经 `vault.imageSource` 就地显示（懒加载）；放行的外链图片按链接呈现、不远程加载；
 * - 嵌入 `![[…]]`：图片就地显示、笔记成块显示内容、其它文件是可点的名字。
 */

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { MarkdownHost } from "../host";
import { embedTargetsOf, splitImageWidth } from "../markdown/markdown";
import { embedSource, MarkdownContent } from "../render/MarkdownContent";
import { openImageLightbox } from "./image-lightbox";
import type { EmbedHit, ReadingVault } from "./reading-vault";
import { frontmatterEntries, splitFrontmatter, splitReadingInline } from "./reading-view";
import { WIKI_LINK_RE } from "./wiki-link-syntax";

const NO_TARGETS: ReadonlySet<string> = new Set();

/** 嵌入里再有嵌入时只显示成链接：不无限展开，也不会互相嵌入成环。 */
const MAX_EMBED_DEPTH = 1;

/**
 * 正文里指不到笔记的 wiki-link 目标（按宿主的回答，与编辑面同一份解析）。没问到之前一律按已解析
 * 呈现——闪一下「未创建」比晚一拍着色更误导。
 */
function useUnresolvedWikiTargets(vault: ReadingVault, notePath: string | null, body: string): ReadonlySet<string> {
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
    void vault.resolveWikiTargets(notePath, targets).then((answer) => {
      if (active && answer) {
        setUnresolved(new Set(targets.filter((_, i) => answer[i] === null)));
      }
    });
    return () => {
      active = false;
    };
  }, [vault, notePath, key]);

  return unresolved;
}

/**
 * 正文里每条嵌入 `![[…]]` 指向什么（宿主解析）。导出时由调用方预先解析好传进来（静态渲染等不到
 * 异步结果）；嵌入里的嵌入不解析。没问到之前一律当「还不知道」，按链接显示。
 */
function useEmbedHits(
  vault: ReadingVault,
  notePath: string | null,
  body: string,
  preset: ReadonlyMap<string, EmbedHit> | undefined,
  enabled: boolean,
): ReadonlyMap<string, EmbedHit> {
  const [hits, setHits] = useState<ReadonlyMap<string, EmbedHit>>(() => new Map());
  // 库外删掉 / 放回附件后重新解析：`![[图.png]]` 指不指得到随之变化（PRD-06-012 ③）。
  const assetGeneration = vault.useAssetGeneration();
  const key = useMemo(() => (preset || !enabled ? "" : embedTargetsOf(body).join("\n")), [body, preset, enabled]);
  useEffect(() => {
    let active = true;
    const targets = key ? key.split("\n") : [];
    if (!notePath || targets.length === 0) {
      setHits(new Map());
      return;
    }
    void vault.resolveEmbeds(notePath, targets).then((answer) => {
      if (!active || !answer) return;
      const known = new Map<string, EmbedHit>();
      targets.forEach((target, i) => {
        const hit = answer[i];
        if (hit) known.set(target, hit);
      });
      setHits(known);
    });
    return () => {
      active = false;
    };
  }, [vault, notePath, key, assetGeneration]);
  return preset ?? hits;
}

interface VaultImageProps {
  vault: ReadingVault;
  notePath: string;
  src: string;
  alt: string;
  width: number | null;
  onOpen: (image: HTMLImageElement) => void;
}

/** 库内相对图片：经宿主解析成可显示的地址；失败退纯文本占位。卸载或换图时释放。 */
function VaultImage({ vault, notePath, src, alt, width, onOpen }: VaultImageProps) {
  const [resolved, setResolved] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  // 库外删掉 / 放回这张图之后重新解析（PRD-06-012 ③）：已显示的图变成断链，断链的图重新出现。
  const assetGeneration = vault.useAssetGeneration();

  useEffect(() => {
    let active = true;
    let release: (() => void) | undefined;
    setResolved(null);
    setFailed(false);
    vault
      .imageSource(notePath, src)
      .then((source) => {
        if (active) {
          release = source.release;
          setResolved(source.url);
        } else {
          source.release?.();
        }
      })
      .catch(() => {
        if (active) setFailed(true);
      });
    return () => {
      active = false;
      release?.();
    };
  }, [vault, notePath, src, assetGeneration]);

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
  host: MarkdownHost;
  vault: ReadingVault;
  content: string;
  /** 当前笔记的库内路径（相对图片与链接的解析锚点）。 */
  notePath: string | null;
  onOpenPath?: (path: string) => void;
  /**
   * 库内链接打开前的拦截：返回 `true` = 已处理（桌面知识产物阅读视图用它把声明出处交给 Rust 按定位锚
   * 打开）；`false` 或缺省走普通的「打开那一篇」。
   */
  interceptVaultLink?: (href: string, occurrence: number) => Promise<boolean>;
  onOpenTag?: (tag: string) => void;
  /**
   * 点一条 wiki-link（`target` 为 `|` 之前的原文）。编辑器传入它，与编辑面同一个动作（打开 / 跳到
   * 片段 / 新建）；缺省时只打开指得到的那一篇、不新建（只读的阅读面）。
   */
  onActivateWikiLink?: (target: string) => void;
  /**
   * 静态导出形态（HTML 导出 / 打印，PRD-11-011）：图片保留笔记相对 src，wiki-link / 标签退成不可点的
   * 文本——导出物里没有「打开另一篇」这回事。渲染基线仍是同一份 `MarkdownContent`。
   */
  staticExport?: boolean;
  /** 预先解析好的嵌入（导出 / 打印：静态渲染等不到异步结果）。缺省时自己问宿主。 */
  embeds?: ReadonlyMap<string, EmbedHit>;
  /** 嵌入层级：被嵌入的笔记以 1 渲染，它里面的嵌入只显示成链接。 */
  embedDepth?: number;
  /** frontmatter 属性块一开始收起（网页，PRD-21-012）；缺省展开（桌面）。 */
  collapseProperties?: boolean;
}

export function ReadingView({
  host,
  vault,
  content,
  notePath,
  onOpenPath,
  interceptVaultLink,
  onOpenTag,
  onActivateWikiLink,
  staticExport = false,
  embeds,
  embedDepth = 0,
  collapseProperties = false,
}: ReadingViewProps) {
  const { t } = useTranslation("editor-core");
  const { frontmatter, body } = useMemo(() => splitFrontmatter(content), [content]);
  const properties = useMemo(() => (frontmatter ? frontmatterEntries(frontmatter) : []), [frontmatter]);

  const unresolved = useUnresolvedWikiTargets(vault, staticExport ? null : notePath, body);
  const embedHits = useEmbedHits(vault, notePath, body, embeds, !staticExport && embedDepth < MAX_EMBED_DEPTH);

  function activateWikiLink(target: string) {
    if (onActivateWikiLink) {
      onActivateWikiLink(target);
      return;
    }
    if (notePath && onOpenPath) vault.openWikiLink(notePath, target, onOpenPath);
  }

  /**
   * 库内相对链接 → 打开那一篇。解析（`../` 归一、越界判定、存不存在）全在宿主的 Rust 里：前端拼一遍
   * 就是第二处会漂的定义。指不到就**什么都不做**——不猜一个"大概是这篇"。
   */
  function openVaultLink(href: string, occurrence: number) {
    if (!notePath || !onOpenPath) return;
    const open = () => vault.openVaultLink(notePath, href, onOpenPath);
    if (!interceptVaultLink) {
      open();
      return;
    }
    void interceptVaultLink(href, occurrence).then((handled) => {
      if (!handled) open();
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
              if (staticExport || !onOpenTag) {
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
                  onClick={() => onOpenTag(part.tag)}
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
    const images = Array.from(
      image.closest(".luna-reading")?.querySelectorAll<HTMLImageElement>("img.luna-reading__image") ?? [],
    );
    openImageLightbox(
      images.map((each) => ({ src: each.src, alt: each.alt })),
      images.indexOf(image),
      {
        dialog: t("reading.lightbox.dialog"),
        close: t("reading.lightbox.close"),
        previous: t("reading.lightbox.previous"),
        next: t("reading.lightbox.next"),
      },
    );
  }

  function renderImage(src: string, alt: string, key: number, width: number | null): ReactNode {
    if (host.isExternalUrlAllowed(src)) {
      if (staticExport) {
        return (
          <a key={key} href={src} title={src}>
            {alt || src}
          </a>
        );
      }
      // 外链图片不远程加载：按链接语义交宿主打开（与对话一致）。
      return (
        <a
          key={key}
          href={src}
          title={src}
          onClick={(event) => {
            event.preventDefault();
            host.openExternalUrl(src);
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
    return <VaultImage key={key} vault={vault} notePath={notePath} src={src} alt={alt} width={width} onOpen={openLightbox} />;
  }

  /** 嵌入照链接显示（嵌入里的嵌入、还没解析出来、行内的笔记嵌入）：点一下打开那一篇。 */
  function embedAsLink(embed: { target: string; display: string | null }, key: number): ReactNode {
    const label = embed.display ?? embed.target;
    if (staticExport) {
      return (
        <span key={key} className="luna-wiki-link luna-reading__wiki">
          {label}
        </span>
      );
    }
    return (
      <button
        key={key}
        type="button"
        className="luna-wiki-link luna-reading__wiki"
        title={embed.target}
        onClick={() => activateWikiLink(embed.target)}
      >
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
          <span key={key} className="luna-embed-missing" title={t("reading.embedMissing")}>
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
                  {t("reading.embedOpen")}
                </button>
              )}
            </div>
            <ReadingView
              host={host}
              vault={vault}
              content={hit.content}
              // 被嵌入的内容里的相对链接已按宿主改写，故按宿主解析图片。
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
        <details className="luna-reading__props" open={!collapseProperties}>
          <summary className="luna-reading__props-title">{t("reading.properties")}</summary>
          <dl>
            {properties.map((entry) => (
              <div key={entry.key} className="luna-reading__prop">
                <dt>{entry.key}</dt>
                <dd>{entry.value || "—"}</dd>
              </div>
            ))}
          </dl>
        </details>
      ) : null}
      <MarkdownContent
        host={host}
        content={body}
        renderText={renderText}
        renderImage={renderImage}
        // 库内相对链接（自动生成的「来源」区逐条如此）：解析归宿主，这里只负责打开。
        // 导出物里没有"打开另一篇"这回事，故静态导出不给这条通道。
        openVaultLink={staticExport || !notePath || !onOpenPath ? undefined : openVaultLink}
        renderEmbed={notePath ? renderEmbed : undefined}
      />
    </div>
  );
}
