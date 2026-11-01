/**
 * ⌘ 悬停链接预览（对标方案 C1；Obsidian「页面预览」）：按住 ⌘（Windows / Linux 为 Ctrl）把指针
 * 停在一条库内链接上——或先停上去再按 ⌘——弹出目标笔记的只读预览，不离开当前这篇。
 *
 * - 认得的链接：`[[wiki]]`（编辑面恒在的 `.luna-wiki-link`，阅读面的 wiki 按钮）与库内相对
 *   md 链接（编辑面折叠成文字的那条、阅读面的 `.luna-markdown__link`）。外链不预览。
 * - 指向哪一篇、存不存在全问 Rust（与点击打开同一条命令，ADR-003）；`[[笔记#标题]]` 预览滚到
 *   那个标题。指不到时说明「还没有这篇」，不猜。
 * - 指针移进预览框可以滚动、点里面的链接；移出链接与预览框一小会儿后收起，Esc、点别处、
 *   滚动正文也收起。预览里的链接不再套一层预览。
 *
 * 只挂一个监听在编辑器外层：编辑面与阅读面共用，哪条链接在指针下由 `resolve` 回答。
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { cssVariables } from "../../lib/css-variables";
import { ReadingView } from "./ReadingView";
import { readingHeadings } from "./reading-view";

export interface LinkRef {
  kind: "wiki" | "md";
  /** wiki = `|` 之前的原文；md = 链接目标（href）。 */
  target: string;
}

export interface LinkPreviewRequest extends LinkRef {
  notePath: string;
  anchor: { left: number; top: number; bottom: number };
}

/** 指针下可能是链接的元素（编辑面与阅读面的几种写法）。 */
const LINK_SELECTOR = ".luna-wiki-link, .luna-md-linkbody, .luna-markdown__link";
/** 移出链接与预览框之后多久收起：留出把指针从链接挪进预览框的时间。 */
const CLOSE_DELAY_MS = 300;

/** 阅读面的链接把目标写在 data 属性上；编辑面的链接由调用方从文档里取。 */
export function linkRefFromDataset(element: Element): LinkRef | null {
  const link = element.closest<HTMLElement>("[data-link-target]");
  const kind = link?.dataset.linkKind;
  const target = link?.dataset.linkTarget;
  return link && target && (kind === "wiki" || kind === "md") ? { kind, target } : null;
}

const isModifier = (event: { metaKey: boolean; ctrlKey: boolean }) => event.metaKey || event.ctrlKey;

export interface LinkPreviewController {
  request: LinkPreviewRequest | null;
  close: () => void;
  /** 指针进出预览框：进去就别收，出来照常倒计时。 */
  holdOpen: () => void;
  releaseHold: () => void;
}

export function useLinkPreview(
  hostRef: RefObject<HTMLElement | null>,
  notePath: string | null,
  resolve: (element: Element) => LinkRef | null,
): LinkPreviewController {
  const [request, setRequest] = useState<LinkPreviewRequest | null>(null);
  const hovered = useRef<Element | null>(null);
  const closeTimer = useRef<number | null>(null);
  const resolveRef = useRef(resolve);
  resolveRef.current = resolve;

  const cancelClose = useCallback(() => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
  }, []);
  const close = useCallback(() => {
    cancelClose();
    setRequest(null);
  }, [cancelClose]);
  const scheduleClose = useCallback(() => {
    if (closeTimer.current !== null) return;
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null;
      setRequest(null);
    }, CLOSE_DELAY_MS);
  }, []);

  useEffect(() => {
    const host = hostRef.current;
    if (!host || !notePath) return;
    const open = (link: Element) => {
      const ref = resolveRef.current(link);
      if (!ref) return;
      cancelClose();
      const rect = link.getBoundingClientRect();
      setRequest((current) =>
        current && current.kind === ref.kind && current.target === ref.target && current.notePath === notePath
          ? current
          : { ...ref, notePath, anchor: { left: rect.left, top: rect.top, bottom: rect.bottom } },
      );
    };
    const onMove = (event: MouseEvent) => {
      const link = event.target instanceof Element ? event.target.closest(LINK_SELECTOR) : null;
      hovered.current = link;
      if (link && isModifier(event)) open(link);
      else if (!link) scheduleClose();
    };
    const onLeave = (event: MouseEvent) => {
      hovered.current = null;
      // 指针去的正是预览框：不排收起。框是挂在 body 下的传送门，进框必先离开宿主；而 React 的
      // onMouseEnter 由 mouseout 合成，比宿主原生的 mouseleave **先**到——等这里再排收起，框上
      // 那次 holdOpen 早已用过，300ms 后框照样关掉（真机 2026-10-04：移不进预览框）。
      if (event.relatedTarget instanceof Element && event.relatedTarget.closest(".luna-link-preview")) return;
      scheduleClose();
    };
    const onKey = (event: KeyboardEvent) => {
      if ((event.key === "Meta" || event.key === "Control") && hovered.current) open(hovered.current);
    };
    host.addEventListener("mousemove", onMove);
    host.addEventListener("mouseleave", onLeave);
    window.addEventListener("keydown", onKey);
    return () => {
      host.removeEventListener("mousemove", onMove);
      host.removeEventListener("mouseleave", onLeave);
      window.removeEventListener("keydown", onKey);
    };
  }, [hostRef, notePath, cancelClose, scheduleClose]);

  useEffect(() => cancelClose, [cancelClose]);
  useEffect(() => close, [notePath, close]);

  return { request, close, holdOpen: cancelClose, releaseHold: scheduleClose };
}

type PreviewState =
  | { status: "loading" }
  | { status: "missing" }
  | { status: "ready"; path: string; content: string; headingIndex: number | null };

/** 问 Rust 链接指向哪一篇，再读出正文。 */
async function loadPreview(request: LinkPreviewRequest): Promise<PreviewState> {
  let path: string | null = null;
  let headingIndex: number | null = null;
  if (request.kind === "wiki") {
    const hit = await commands.vaultResolveWikilink(request.notePath, request.target);
    if (hit.status !== "ok" || !hit.data.exists) return { status: "missing" };
    path = hit.data.path;
    headingIndex = hit.data.heading_index;
  } else {
    const hit = await commands.vaultLinkNoteTarget(request.notePath, request.target);
    path = hit.status === "ok" ? hit.data : null;
  }
  if (!path) return { status: "missing" };
  const note = await commands.readNote(path);
  return note.status === "ok" ? { status: "ready", path, content: note.data.content, headingIndex } : { status: "missing" };
}

/** 视口边距：预览框不贴着窗口边。 */
const VIEWPORT_MARGIN = 8;
/** 预览框与链接之间的间隙。 */
const ANCHOR_GAP = 6;

export function LinkPreviewPopover({
  controller,
  onOpenPath,
}: {
  controller: LinkPreviewController;
  onOpenPath: (path: string) => void;
}) {
  const { t } = useTranslation("editor");
  const { request, close, holdOpen, releaseHold } = controller;
  const [state, setState] = useState<PreviewState>({ status: "loading" });
  const [position, setPosition] = useState<{ x: number; y: number } | null>(null);
  const boxRef = useRef<HTMLDivElement | null>(null);
  const bodyRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!request) return;
    let active = true;
    setState({ status: "loading" });
    void loadPreview(request).then((next) => {
      if (active) setState(next);
    });
    return () => {
      active = false;
    };
  }, [request]);

  // 先量预览框，再决定放链接下方还是上方（下方放不下才翻上去），左右夹进视口。框里的图片、
  // 嵌入是渲染之后才加载的，框随之长高：尺寸一变就重新放，否则长出去的那截落到窗口外。
  useLayoutEffect(() => {
    const box = boxRef.current;
    if (!request || !box) {
      setPosition(null);
      return;
    }
    const place = () => {
      const { width, height } = box.getBoundingClientRect();
      const below = request.anchor.bottom + ANCHOR_GAP;
      const y =
        below + height <= window.innerHeight - VIEWPORT_MARGIN
          ? below
          : Math.max(VIEWPORT_MARGIN, request.anchor.top - ANCHOR_GAP - height);
      const x = Math.min(Math.max(VIEWPORT_MARGIN, request.anchor.left), window.innerWidth - width - VIEWPORT_MARGIN);
      setPosition((current) => (current && current.x === x && current.y === y ? current : { x, y }));
    };
    place();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(place);
    observer.observe(box);
    return () => observer.disconnect();
  }, [request, state]);

  // `[[笔记#标题]]`：滚到那个标题。
  useEffect(() => {
    const body = bodyRef.current;
    if (state.status !== "ready" || state.headingIndex === null || !body) return;
    const heading = readingHeadings(body)[state.headingIndex];
    if (heading) body.scrollTop = heading.offsetTop - body.offsetTop;
  }, [state]);

  // Esc、点预览框以外、滚动正文：收起。
  useEffect(() => {
    if (!request) return;
    const outside = (event: Event) => event.target instanceof Node && !boxRef.current?.contains(event.target);
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      close();
    };
    const onPointer = (event: Event) => {
      if (outside(event)) close();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onPointer, true);
    window.addEventListener("scroll", onPointer, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onPointer, true);
      window.removeEventListener("scroll", onPointer, true);
    };
  }, [request, close]);

  if (!request) return null;
  const open = (path: string) => {
    close();
    onOpenPath(path);
  };
  return createPortal(
    <div
      ref={boxRef}
      className="luna-link-preview"
      role="dialog"
      aria-label={t("linkPreview.label")}
      data-placed={position ? "true" : undefined}
      style={position ? cssVariables({ "--link-preview-x": `${position.x}px`, "--link-preview-y": `${position.y}px` }) : undefined}
      onMouseEnter={holdOpen}
      onMouseLeave={releaseHold}
    >
      {state.status === "ready" ? (
        <>
          <header className="luna-link-preview__head">
            <span className="luna-link-preview__path" title={state.path}>
              {state.path}
            </span>
            <button type="button" className="luna-link-preview__open" onClick={() => open(state.path)}>
              {t("linkPreview.open")}
            </button>
          </header>
          <div ref={bodyRef} className="luna-link-preview__body">
            <ReadingView content={state.content} notePath={state.path} onOpenPath={open} />
          </div>
        </>
      ) : (
        <p className="luna-link-preview__status">
          {state.status === "loading" ? t("linkPreview.loading") : t("linkPreview.missing")}
        </p>
      )}
    </div>,
    document.body,
  );
}
