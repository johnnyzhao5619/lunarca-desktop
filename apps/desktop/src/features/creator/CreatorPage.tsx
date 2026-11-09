/**
 * 创作空间各页共用的页面骨架（ADR-165 ⑨ 应用内统一规范）：
 *
 * - **页头**一个组件：面包屑（「返回」只在这里）、标题与一句状态、至多两个次要动作、一个主动作。
 *   看板、项目工作区、灵感库、灵感条目都用它，不再各写一份。
 * - **即时操作的结果就近说**：页头下面一条提示（`PageNotice`），不进全局状态栏——状态栏只放
 *   全局状态。后台任务照旧进处理队列与通知中心。
 * - **加载中**一种样子（`PageLoading`）；「没有」才用 `EmptyState`。
 */

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { ChevronRightIcon, CloseIcon } from "@lunarca/editor-core/Icon";
import { IconButton } from "../../components/ui/IconButton";

export interface CreatorPageHeaderProps {
  /** 上一级（面包屑）；顶层页没有。 */
  crumb?: { label: string; onClick: () => void };
  title: string;
  titleId?: string;
  /** 标题旁的一句状态（如「共 12 条」）。 */
  status?: ReactNode;
  /** 次要动作（至多两个）。 */
  secondary?: ReactNode;
  /** 唯一的主动作。 */
  primary?: ReactNode;
}

export function CreatorPageHeader({ crumb, title, titleId, status, secondary, primary }: CreatorPageHeaderProps) {
  const { t } = useTranslation("workspace");
  return (
    <header className="luna-main-header">
      <div className="luna-main-header__title">
        {crumb ? (
          <nav className="luna-creator-page__crumb" aria-label={t("creator.page.crumbLabel")}>
            <button type="button" className="luna-creator-page__crumb-link" onClick={crumb.onClick}>
              {crumb.label}
            </button>
            <ChevronRightIcon size={12} />
          </nav>
        ) : null}
        <h1 id={titleId}>{title}</h1>
        {status ? <span className="luna-creator-page__status">{status}</span> : null}
      </div>
      {secondary || primary ? (
        <div className="luna-main-header__actions">
          {secondary}
          {primary}
        </div>
      ) : null}
    </header>
  );
}

/** 一条就近的结果提示。`error` 用警示色并以 `alert` 播报。 */
export interface PageNoticeValue {
  text: string;
  tone: "info" | "error";
}

export function PageNotice({ notice, onDismiss }: { notice: PageNoticeValue | null; onDismiss: () => void }) {
  const { t } = useTranslation("workspace");
  if (!notice) return null;
  return (
    <div className="luna-creator-page__notice" role={notice.tone === "error" ? "alert" : "status"} data-tone={notice.tone}>
      <span>{notice.text}</span>
      <IconButton density="sm" icon={<CloseIcon size={12} />} label={t("creator.page.dismiss")} onClick={onDismiss} />
    </div>
  );
}

const noticeListeners = new Set<(notice: PageNoticeValue) => void>();

/** 换页时还在路上的那一句：动作常常同时换页（删掉正看着的项目 → 回看板），说给旧页就丢了。 */
let pending: { notice: PageNoticeValue; at: number } | null = null;

/** 换页之后多久内新页还接这一句（毫秒）；过了就算过时，不再翻出来。 */
const PENDING_NOTICE_MS = 1500;

/**
 * 从页外（侧栏、项目菜单）说一句结果：当前显示着的那一页的提示条接住它；紧接着换上来的那一页
 * 也接住（见 `pending`）。创作同一时刻只显示一页，所以不会两处同时说。
 */
export function postPageNotice(notice: PageNoticeValue): void {
  pending = { notice, at: Date.now() };
  for (const listener of noticeListeners) listener(notice);
}

function takePending(): PageNoticeValue | null {
  const taken = pending !== null && Date.now() - pending.at <= PENDING_NOTICE_MS ? pending.notice : null;
  pending = null;
  return taken;
}

/** 页内提示的状态：`fail` 说失败、`tell` 说结果、`clear` 收起；也接住 `postPageNotice`。 */
export function usePageNotice() {
  const [notice, setNotice] = useState<PageNoticeValue | null>(null);
  useEffect(() => {
    const arrived = takePending();
    if (arrived) setNotice(arrived);
    noticeListeners.add(setNotice);
    return () => {
      noticeListeners.delete(setNotice);
    };
  }, []);
  const fail = useCallback((text: string) => setNotice({ text, tone: "error" }), []);
  const tell = useCallback((text: string) => setNotice({ text, tone: "info" }), []);
  const clear = useCallback(() => setNotice(null), []);
  return { notice, fail, tell, clear };
}

/** 加载中。 */
export function PageLoading() {
  const { t } = useTranslation("workspace");
  return (
    <p className="luna-creator-page__loading" role="status">
      {t("common:status.loading")}
    </p>
  );
}
