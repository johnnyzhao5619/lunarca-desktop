/**
 * 灵感库页（ADR-162 ⑬，PRD-19-005）：灵感按「套路」分组列出；「归类套路」先出提案、改好再写回。
 *
 * 分组、摘句、标签枝与写回全在 Rust（`creator_swipe_library` / `creator_suggest_swipe_patterns` /
 * `creator_apply_swipe_patterns`）；提案在写回前只存在于这一页，改类名、挪出条目都只是改这份
 * 草稿，这里只渲染与发命令（ADR-003）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type SwipeItemDto,
  type SwipeLibraryDto,
  type SwipeProposalDto,
} from "../../ipc/bindings";
import { EmptyState } from "../../components/ui/EmptyState";
import { BookmarkIcon, CloseIcon } from "../../components/ui/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { vaultErrorText } from "../../lib/vault-error";
import { CreatorPageHeader, PageLoading, PageNotice, usePageNotice } from "./CreatorPage";

/** 提案草稿：类名可改、条目可挪出。 */
interface DraftGroup {
  name: string;
  members: SwipeItemDto[];
}

interface Draft {
  groups: DraftGroup[];
  considered: number;
  total: number;
}

function draftOf(proposal: SwipeProposalDto): Draft {
  return {
    groups: proposal.groups.map((group) => ({ name: group.name, members: group.members })),
    considered: proposal.considered,
    total: proposal.total,
  };
}

export interface SwipeLibraryProps {
  onBack: () => void;
  onOpenNote: (path: string) => void;
  onCreateSwipe: () => void;
}

export function SwipeLibrary({ onBack, onOpenNote, onCreateSwipe }: SwipeLibraryProps) {
  const { t } = useTranslation("workspace");
  const [library, setLibrary] = useState<SwipeLibraryDto | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const { notice, fail, tell, clear } = usePageNotice();
  const [loadFailed, setLoadFailed] = useState(false);
  const requestRef = useRef(0);

  const load = () => {
    const request = ++requestRef.current;
    void commands.creatorSwipeLibrary().then((reply) => {
      if (request !== requestRef.current) return;
      if (reply.status !== "ok") {
        setLoadFailed(true);
        fail(vaultErrorText(reply.error));
        return;
      }
      setLoadFailed(false);
      setLibrary(reply.data);
    });
  };

  useEffect(load, []);
  // 新建、改了标签、删了灵感：重取一次。
  useVaultReload(load);

  const suggest = async () => {
    setBusy(true);
    clear();
    const reply = await commands.creatorSuggestSwipePatterns();
    setBusy(false);
    if (reply.status !== "ok") {
      fail(vaultErrorText(reply.error));
      return;
    }
    setDraft(draftOf(reply.data));
  };

  const apply = async () => {
    if (!draft) return;
    setBusy(true);
    clear();
    const reply = await commands.creatorApplySwipePatterns(
      draft.groups
        .filter((group) => group.members.length > 0)
        .map((group) => ({ name: group.name, paths: group.members.map((member) => member.path) })),
    );
    setBusy(false);
    if (reply.status !== "ok") {
      fail(vaultErrorText(reply.error));
      return;
    }
    setDraft(null);
    tell(
      reply.data.skipped.length > 0
        ? t("creator.swipeLibrary.appliedWithSkips", {
            count: reply.data.written,
            skipped: reply.data.skipped.length,
          })
        : t("creator.swipeLibrary.applied", { count: reply.data.written }),
    );
    load();
  };

  const rename = (index: number, name: string) =>
    setDraft((current) =>
      current
        ? { ...current, groups: current.groups.map((group, i) => (i === index ? { ...group, name } : group)) }
        : current,
    );

  const removeMember = (index: number, path: string) =>
    setDraft((current) =>
      current
        ? {
            ...current,
            groups: current.groups.map((group, i) =>
              i === index ? { ...group, members: group.members.filter((member) => member.path !== path) } : group,
            ),
          }
        : current,
    );

  const item = (entry: SwipeItemDto) => (
    <button type="button" className="luna-creator-page__item" title={entry.path} onClick={() => onOpenNote(entry.path)}>
      <span className="luna-creator-page__item-title">{entry.title}</span>
      {entry.why ? <span className="luna-creator-page__item-meta">{entry.why}</span> : null}
    </button>
  );

  return (
    <section className="luna-creator">
      <CreatorPageHeader
        crumb={{ label: t("nav.creator"), onClick: onBack }}
        title={t("creator.swipeLibrary.title")}
        status={library ? t("creator.swipeLibrary.count", { count: library.total }) : null}
        secondary={
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy || draft !== null}
            onClick={() => void suggest()}
          >
            {t(busy && !draft ? "creator.swipeLibrary.suggesting" : "creator.swipeLibrary.suggest")}
          </button>
        }
        primary={
          <button type="button" className="luna-btn luna-btn--primary" onClick={onCreateSwipe}>
            {t("creator.newSwipe")}
          </button>
        }
      />
      <PageNotice notice={notice} onDismiss={clear} />

      <div className="luna-creator-page__body">

        {draft ? (
          <section className="luna-creator-page__section" aria-labelledby="swipe-proposal">
            <h2 id="swipe-proposal">{t("creator.swipeLibrary.proposal")}</h2>
            <p className="luna-creator-page__hint">
              {draft.considered < draft.total
                ? t("creator.swipeLibrary.proposalPartial", { considered: draft.considered, total: draft.total })
                : t("creator.swipeLibrary.proposalHint")}
            </p>
            {draft.groups.map((group, index) => (
              <div key={index} className="luna-creator-page__field">
                <input
                  className="luna-input"
                  value={group.name}
                  aria-label={t("creator.swipeLibrary.groupName", { index: index + 1 })}
                  onChange={(event) => rename(index, event.target.value)}
                />
                <ul className="luna-creator-page__list">
                  {group.members.map((member) => (
                    <li key={member.path} className="luna-creator-page__row">
                      <span className="luna-creator-page__item-title">{member.title}</span>
                      <IconButton
                        density="sm"
                        label={t("creator.swipeLibrary.removeMember", { title: member.title })}
                        icon={<CloseIcon size={12} />}
                        onClick={() => removeMember(index, member.path)}
                      />
                    </li>
                  ))}
                </ul>
              </div>
            ))}
            <div className="luna-creator-page__row">
              <button type="button" className="luna-btn luna-btn--primary luna-btn--sm" disabled={busy} onClick={() => void apply()}>
                {t("creator.swipeLibrary.apply")}
              </button>
              <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" disabled={busy} onClick={() => setDraft(null)}>
                {t("creator.swipeLibrary.discard")}
              </button>
            </div>
          </section>
        ) : null}

        {library === null && !loadFailed ? <PageLoading /> : null}
        {library && library.total === 0 ? (
          <EmptyState
            icon={<BookmarkIcon size={24} />}
            title={t("creator.swipesEmptyTitle")}
            description={t("creator.swipesEmptyDescription")}
          />
        ) : null}
        {library?.groups.map((group) => {
          const id = `swipe-group-${group.name ?? "loose"}`;
          return (
            <section key={id} className="luna-creator-page__section" aria-labelledby={id}>
              <h2 id={id}>{group.name ?? t("creator.swipeLibrary.loose")}</h2>
              <ul className="luna-creator-page__list">
                {group.items.map((entry) => (
                  <li key={entry.path}>{item(entry)}</li>
                ))}
              </ul>
            </section>
          );
        })}
      </div>
    </section>
  );
}
