/**
 * 知识产物只读阅读视图（ADR-096 ③ / TDD-07 §4.1d）：「知识库 → 主题页」的主区。
 *
 * **只读呈现，不是第二张编辑桌**（PRD-07-058 不动）：正文渲染**复用编辑器阅读模式的
 * 同一个 `ReadingView`**（TrashPreview 同一先例）——不为产物另写一套 Markdown 渲染，
 * 两套渲染的分叉是「同一篇在两处长得不一样」而界面上看不出错。
 *
 * **落位在 `features/workspace/`**：它组合 `editor/ReadingView` 与 `knowledge/TopicBadge`
 * 两个 feature 的件，而 `editor → knowledge` 已有边（徽标等四件）——放进 knowledge 会成环
 * （`check-frontend-structure.mjs` 禁）。workspace → editor / knowledge 两条边向都已存在
 * （TrashPreview / TagsView 先例），壳层组合件本就归这里。
 *
 * 工具条只挂既有能力：徽标（含立即归入菜单）/ 全文朗读（起停；精细控制条是编辑器的
 * 精装，不搬）/ 版本历史 / 在文件管理器中显示。「转为我的笔记」与复核态两出口归 D3。
 */

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type TopicClaimFilterDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import {
  ChevronLeftIcon,
  ClockIcon,
  FolderIcon,
  ListIcon,
  SpeakIcon,
} from "@lunarca/editor-core/Icon";
import { EmptyState } from "../../components/ui/EmptyState";
import { vaultErrorText } from "../../lib/vault-error";
import { ReadingView } from "../editor/ReadingView";
import { VersionHistoryPanel } from "../editor/VersionHistoryPanel";
import { parseOutline } from "../editor/outline";
import { useTts, useTtsAvailable } from "../tts/use-tts";
import { TopicBadge } from "../knowledge/TopicBadge";
import { TopicEvidencePanel } from "../knowledge/TopicEvidencePanel";
import { TopicReadingSummary } from "../knowledge/TopicReadingSummary";
import { TopicClaimFilterList } from "../knowledge/TopicClaimFilterList";
import { recallReadingScroll, rememberReadingScroll } from "../knowledge/reading-memory";
import { noteStem } from "../../lib/note-path";
import { reportCitationOutcome } from "../../lib/app-events";
import { readingHeadings } from "@lunarca/editor-core/reading-view";

/** 标题少于此数的短页不给大纲入口：一屏看得完的页不需要目录（TDD-07 §4.1e）。 */
const OUTLINE_MIN_HEADINGS = 4;

export interface KnowledgeReadingViewProps {
  /** 要呈现的产物路径。未选中页的空选态由壳层渲染工作台（`KnowledgeWorkbench`），
      不进本组件——本组件只负责「读一页」。 */
  path: string;
  /** 正文里的链接点击（源笔记 → 编辑桌；产物 → 回本视图。分流在壳层闸口）。 */
  onOpenPath: (path: string) => void;
  onOpenTag: (tag: string) => void;
  onStatus: (status: string) => void;
  /** 回到知识库首页（主题页未选中态）：选中一页后壳层别无回去的路，入口必须在页内。 */
  onBack: () => void;
}

export function KnowledgeReadingView({
  path,
  onOpenPath,
  onOpenTag,
  onStatus,
  onBack,
}: KnowledgeReadingViewProps) {
  const { t } = useTranslation("workspace");
  const { t: tEditor } = useTranslation("editor");
  const { t: tCommon } = useTranslation("common");
  const [content, setContent] = useState<string | null>(null);
  /** 读取失败的原因（已本地化）；`null` = 没有失败。失败与「还在读」是两句不同的话。 */
  const [loadError, setLoadError] = useState<string | null>(null);
  /** 当前 `content` 属于哪一页：切页时旧正文还在，滚动恢复不能用它冒充新页。 */
  const contentPathRef = useRef<string | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [outlineOpen, setOutlineOpen] = useState(false);
  /** 「只看分歧 / 只看需核对」；`null` = 全文。切页即回到全文。 */
  const [claimFilter, setClaimFilter] = useState<TopicClaimFilterDto | null>(null);
  /** 最近一次 `load`（`useVaultReload` 恒调最新那一份；路径切换后旧的置空即失效）。 */
  const loadRef = useRef<(() => void) | null>(null);
  // 朗读：单一发声者（TDD-05 §9），本视图发起的会话记 session 认领——切页/卸载即停。
  const { state: ttsState, speak: ttsSpeak, stop: ttsStop } = useTts();
  const ttsAvailable = useTtsAvailable();
  const [readSessionId, setReadSessionId] = useState(0);
  const readActive =
    readSessionId !== 0 &&
    ttsState.session_id === readSessionId &&
    (ttsState.status === "speaking" || ttsState.status === "paused");

  useEffect(() => {
    setHistoryOpen(false);
    setOutlineOpen(false);
    setClaimFilter(null);
    setLoadError(null);
    if (readActive) {
      void ttsStop();
    }
    // 切页停朗读是本效果唯一的清理点；readActive 刻意不进依赖——"朗读结束"不该触发空跑。
  }, [path]);

  useEffect(() => {
    let disposed = false;
    const load = () => {
      void commands.readNote(path).then((result) => {
        if (!disposed) {
          contentPathRef.current = path;
          setContent(result.status === "ok" ? result.data.content : null);
          setLoadError(result.status === "ok" ? null : vaultErrorText(result.error));
        }
      });
    };
    load();
    loadRef.current = load;
    // 记为工作台「继续阅读」的目标；Rust 只收知识产物，失败不影响阅读。
    void commands.recordKnowledgeVisit(path);
    return () => {
      disposed = true;
      loadRef.current = null;
    };
  }, [path]);

  // 产物是**系统会持续改写**的内容（归入直写、地图刷新）：正文必须跟着写盘刷新，
  // 否则用户盯着的是一页已经过期的快照。合并同一阵变更的实现只有一份
  // （`use-vault-reload`）——这里此前是自己抄的一份 `setTimeout` + 一个同名常量。
  useVaultReload(() => loadRef.current?.());

  const title = useMemo(() => noteStem(path), [path]);
  // 与编辑器大纲同一解析（代码块内伪标题天然排除）；`headingIndex` 即正文里第几个标题。
  const outline = useMemo(() => (content === null ? [] : parseOutline(content)), [content]);

  // 回到这一页时接着上次读到的位置（点出处去看原文、再回来是高频路径）。
  const bodyRef = useRef<HTMLDivElement>(null);
  const restoredPathRef = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (content === null || contentPathRef.current !== path || restoredPathRef.current === path) {
      return;
    }
    restoredPathRef.current = path;
    const top = recallReadingScroll(path);
    if (bodyRef.current && top !== undefined) {
      bodyRef.current.scrollTop = top;
    }
  }, [content, path]);

  // 进出筛选：清单从顶部看起，回到全文时接着刚才读到的位置。
  useLayoutEffect(() => {
    if (!bodyRef.current || contentPathRef.current !== path) {
      return;
    }
    bodyRef.current.scrollTop = claimFilter === null ? (recallReadingScroll(path) ?? 0) : 0;
  }, [claimFilter]);

  return (
    <section className="luna-knowledge-reading" aria-label={title}>
      <header className="luna-knowledge-reading__header">
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onBack}>
          <ChevronLeftIcon size={13} />
          <span>{t("topics.home")}</span>
        </button>
        <h1 className="luna-knowledge-reading__title">{title}</h1>
        {/* 身份徽标 + 「立即归入」菜单：与此前编辑器页头**同一个组件**（宿主随产物迁来）。
            阅读视图没有上下文面板，「查看喂养来源」项由组件按缺省自行不给。 */}
        <TopicBadge notePath={path} />
        <span className="luna-knowledge-reading__tools">
          {ttsAvailable && (
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              data-speaking={readActive || undefined}
              onClick={() => {
                if (readActive) {
                  void ttsStop();
                  return;
                }
                if (content !== null) {
                  void ttsSpeak({ text: content, markdown: true, scope: `knowledge:${path}` }).then(
                    (next) => setReadSessionId(next.session_id),
                  );
                }
              }}
            >
              <SpeakIcon size={13} />
              <span>{readActive ? tEditor("read.stopDoc") : tEditor("read.readDoc")}</span>
            </button>
          )}
          {claimFilter === null && outline.length >= OUTLINE_MIN_HEADINGS && (
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              aria-expanded={outlineOpen}
              onClick={() => setOutlineOpen((open) => !open)}
            >
              <ListIcon size={13} />
              <span>{t("panel.outline")}</span>
            </button>
          )}
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => setHistoryOpen(true)}
          >
            <ClockIcon size={13} />
            <span>{tEditor("version.open")}</span>
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => {
              void commands.revealVaultPath(path).then((result) => {
                if (result.status === "error") {
                  onStatus(result.error);
                }
              });
            }}
          >
            <FolderIcon size={13} />
            <span>{tCommon("menu.revealInFileManager")}</span>
          </button>
        </span>
      </header>
      {/* 范围 → 声明数 → 需核对的出处（ADR-134）；旧格式没有汇总，不渲染。 */}
      <TopicReadingSummary
        path={path}
        onOpenPath={onOpenPath}
        onOpenTag={onOpenTag}
        filter={claimFilter}
        onFilterChange={setClaimFilter}
      />
      {/* 持续整理入口（ADR-134）：只在维护就绪或旧格式时出现，其余状态由徽标承担。 */}
      <TopicEvidencePanel
        path={path}
        onOpenPath={onOpenPath}
        onOpenHistory={() => setHistoryOpen(true)}
      />
      {outlineOpen && (
        <nav className="luna-knowledge-reading__outline" aria-label={t("panel.outline")}>
          <ul className="luna-outline">
            {outline.map((heading) => (
              <li key={`${heading.line}-${heading.headingIndex}`}>
                <button
                  type="button"
                  className="luna-outline__item"
                  data-level={heading.level}
                  title={heading.text}
                  onClick={() => {
                    const body = bodyRef.current;
                    if (body) readingHeadings(body)[heading.headingIndex]?.scrollIntoView({ block: "start" });
                    setOutlineOpen(false);
                  }}
                >
                  {heading.text}
                </button>
              </li>
            ))}
          </ul>
        </nav>
      )}
      <div
        ref={bodyRef}
        className="luna-knowledge-reading__body"
        onScroll={(event) => {
          // 筛选清单的滚动不是正文的阅读位置，不记。
          if (claimFilter === null) {
            rememberReadingScroll(path, event.currentTarget.scrollTop);
          }
        }}
      >
        {claimFilter !== null ? (
          <TopicClaimFilterList
            path={path}
            filter={claimFilter}
            onClear={() => setClaimFilter(null)}
            onOpenPath={onOpenPath}
          />
        ) : loadError !== null ? (
          <EmptyState
            title={t("topics.readError")}
            description={loadError}
            action={
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => loadRef.current?.()}
              >
                {tCommon("action.retry")}
              </button>
            }
          />
        ) : content === null ? (
          <p className="luna-empty" role="status">
            {t("common:status.loading")}
          </p>
        ) : (
          <ReadingView
            content={content}
            notePath={path}
            onOpenPath={onOpenPath}
            interceptVaultLink={async (href, occurrence) => {
              // 声明出处由 Rust 按定位锚、版本优先打开；块外链接退回普通打开。
              const result = await commands.openTopicLink(path, href, occurrence);
              if (result.status !== "ok" || result.data.status !== "opened") {
                return false;
              }
              reportCitationOutcome({ title: result.data.title, outcome: result.data.outcome });
              return true;
            }}
            onOpenTag={onOpenTag}
          />
        )}
      </div>
      {historyOpen && <VersionHistoryPanel path={path} onClose={() => setHistoryOpen(false)} />}
    </section>
  );
}
