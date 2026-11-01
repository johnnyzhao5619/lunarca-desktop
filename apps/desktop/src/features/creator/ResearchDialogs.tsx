/**
 * 项目研究的两个对话框（ADR-167 ⑥，PRD-19-046）：
 *
 * - **来源指南**：一条来源的摘要与要点。打开即取（`creator_source_guide`；正文没变时 Rust 直接给缓存，
 *   不调模型），可「重新生成」。
 * - **写研究简报**：勾选「素材与来源」里的几条（默认全选，至多 12 条），交给 Rust 合成一篇项目里
 *   的新笔记，每条发现链回它出自的来源；写好就在项目里打开它。
 * - **从简报出发**：读项目最新一份研究简报，提三个选题（选一个、可改，写进主页「选题」一节），或
 *   起草一版脚本（脚本还是空骨架就写进它，否则另起一篇「脚本初稿」——落在哪由 Rust 定）。
 *
 * 读来源、分配字数、调模型、解析与落盘都在 Rust（ADR-003）；这里只收集选择、渲染结果。
 */

import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { commands, type OverviewItemDto, type ScriptDraftDto, type SourceGuideDto, type TopicIdeaDto } from "../../ipc/bindings";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { vaultErrorText } from "../../lib/vault-error";
import { PageLoading } from "./CreatorPage";

/** 一份简报最多读几条来源（与 Rust `BRIEF_MAX_SOURCES` 同一个数；超了 Rust 会拒）。 */
const BRIEF_MAX_SOURCES = 12;

export interface SourceGuideDialogProps {
  path: string;
  title: string;
  onClose: () => void;
}

export function SourceGuideDialog({ path, title, onClose }: SourceGuideDialogProps) {
  const { t } = useTranslation("workspace");
  const [guide, setGuide] = useState<SourceGuideDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const closeRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, initialFocusRef: closeRef });
  const requestRef = useRef(0);

  const load = (refresh: boolean) => {
    const request = ++requestRef.current;
    setBusy(true);
    setError(null);
    void commands.creatorSourceGuide(path, refresh).then((reply) => {
      if (request !== requestRef.current) return;
      setBusy(false);
      if (reply.status !== "ok") {
        setError(vaultErrorText(reply.error));
        return;
      }
      setGuide(reply.data);
    });
  };

  useEffect(() => load(false), [path]);

  return createPortal(
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-research"
        role="dialog"
        aria-modal="true"
        aria-labelledby="source-guide-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="source-guide-title">{t("creator.research.guideTitle", { name: title })}</h3>
          <p>{t("creator.research.guideHint")}</p>
        </header>
        {busy && guide === null ? <PageLoading /> : null}
        {guide ? (
          <div className="luna-research__body">
            {guide.summary ? <p className="luna-research__summary">{guide.summary}</p> : null}
            {guide.points.length > 0 ? (
              <ul className="luna-research__points" aria-label={t("creator.research.points")}>
                {guide.points.map((point, index) => (
                  <li key={index}>{point}</li>
                ))}
              </ul>
            ) : null}
            {guide.cached ? <p className="luna-creator-page__hint">{t("creator.research.cached")}</p> : null}
          </div>
        ) : null}
        {error ? (
          <p className="luna-research__error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" disabled={busy} onClick={() => load(true)}>
            {t(busy && guide !== null ? "creator.research.generating" : "creator.research.regenerate")}
          </button>
          <button ref={closeRef} type="button" className="luna-btn luna-btn--primary" onClick={onClose}>
            {t("common:action.close")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

export interface ResearchBriefDialogProps {
  home: string;
  /** 「素材与来源」里的来源（顺序即列出的顺序）。 */
  sources: readonly OverviewItemDto[];
  onClose: () => void;
  /** 简报写好了：在项目里打开它。 */
  onCreated: (path: string) => void;
}

export function ResearchBriefDialog({ home, sources, onClose, onCreated }: ResearchBriefDialogProps) {
  const { t } = useTranslation("workspace");
  const [picked, setPicked] = useState<string[]>(() =>
    sources.slice(0, BRIEF_MAX_SOURCES).map((source) => source.path),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, initialFocusRef: closeRef });

  const toggle = (path: string) =>
    setPicked((current) =>
      current.includes(path)
        ? current.filter((entry) => entry !== path)
        : current.length < BRIEF_MAX_SOURCES
          ? [...current, path]
          : current,
    );

  const generate = async () => {
    setBusy(true);
    setError(null);
    // 按列出的顺序交给 Rust（编号即这个顺序）。
    const ordered = sources.map((source) => source.path).filter((path) => picked.includes(path));
    const reply = await commands.creatorResearchBrief(home, ordered);
    setBusy(false);
    if (reply.status !== "ok") {
      setError(vaultErrorText(reply.error));
      return;
    }
    onCreated(reply.data);
  };

  return createPortal(
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-research"
        role="dialog"
        aria-modal="true"
        aria-labelledby="research-brief-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="research-brief-title">{t("creator.research.briefTitle")}</h3>
          <p>{t("creator.research.briefHint", { max: BRIEF_MAX_SOURCES })}</p>
        </header>
        <ul className="luna-research__sources" aria-label={t("creator.research.sources")}>
          {sources.map((source) => (
            <li key={source.path}>
              <label className="luna-research__source" title={source.path}>
                <input
                  type="checkbox"
                  checked={picked.includes(source.path)}
                  disabled={busy || (!picked.includes(source.path) && picked.length >= BRIEF_MAX_SOURCES)}
                  onChange={() => toggle(source.path)}
                />
                <span>{source.title}</span>
              </label>
            </li>
          ))}
        </ul>
        {error ? (
          <p className="luna-research__error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="luna-dialog__actions">
          <button ref={closeRef} type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
            {t("common:action.cancel")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={busy || picked.length === 0}
            onClick={() => void generate()}
          >
            {t(busy ? "creator.research.writing" : "creator.research.write", { count: picked.length })}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

export interface BriefNextDialogProps {
  home: string;
  onClose: () => void;
  /** 选题已写进主页（主页正开在左边，Rust 广播了新正文）。 */
  onTopicSaved: () => void;
  /** 脚本起草好了：在项目里打开它。 */
  onScriptDrafted: (draft: ScriptDraftDto) => void;
}

export function BriefNextDialog({ home, onClose, onTopicSaved, onScriptDrafted }: BriefNextDialogProps) {
  const { t } = useTranslation("workspace");
  const [busy, setBusy] = useState<"topics" | "script" | "save" | null>(null);
  const [topics, setTopics] = useState<TopicIdeaDto[] | null>(null);
  const [picked, setPicked] = useState(0);
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, initialFocusRef: closeRef });

  const suggest = async () => {
    setBusy("topics");
    setError(null);
    const reply = await commands.creatorBriefTopics(home);
    setBusy(null);
    if (reply.status !== "ok") {
      setError(vaultErrorText(reply.error));
      return;
    }
    setTopics(reply.data);
    setPicked(0);
    setText(reply.data[0]?.title ?? "");
  };

  const pick = (index: number) => {
    setPicked(index);
    setText(topics?.[index]?.title ?? "");
  };

  const save = async () => {
    setBusy("save");
    setError(null);
    const reply = await commands.creatorSetTopic(home, text);
    setBusy(null);
    if (reply.status !== "ok") {
      setError(vaultErrorText(reply.error));
      return;
    }
    onTopicSaved();
  };

  const draft = async () => {
    setBusy("script");
    setError(null);
    const reply = await commands.creatorDraftScript(home);
    setBusy(null);
    if (reply.status !== "ok") {
      setError(vaultErrorText(reply.error));
      return;
    }
    onScriptDrafted(reply.data);
  };

  return createPortal(
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-research"
        role="dialog"
        aria-modal="true"
        aria-labelledby="brief-next-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="brief-next-title">{t("creator.research.nextTitle")}</h3>
          <p>{t("creator.research.nextHint")}</p>
        </header>
        <div className="luna-research__choices">
          <button type="button" className="luna-btn luna-btn--secondary" disabled={busy !== null} onClick={() => void suggest()}>
            {t(busy === "topics" ? "creator.research.suggesting" : topics ? "creator.research.suggestAgain" : "creator.research.suggest")}
          </button>
          <button type="button" className="luna-btn luna-btn--secondary" disabled={busy !== null} onClick={() => void draft()}>
            {t(busy === "script" ? "creator.research.drafting" : "creator.research.draft")}
          </button>
        </div>
        {topics ? (
          <div className="luna-research__body">
            <ul className="luna-research__sources" aria-label={t("creator.research.topics")}>
              {topics.map((topic, index) => (
                <li key={index}>
                  <label className="luna-research__source">
                    <input
                      type="radio"
                      name="brief-topic"
                      checked={picked === index}
                      disabled={busy !== null}
                      onChange={() => pick(index)}
                    />
                    <span className="luna-research__topic">
                      <span>{topic.title}</span>
                      {topic.angle ? <span className="luna-research__angle">{topic.angle}</span> : null}
                    </span>
                  </label>
                </li>
              ))}
            </ul>
            <input
              className="luna-input"
              aria-label={t("creator.research.topicText")}
              value={text}
              disabled={busy !== null}
              onChange={(event) => setText(event.target.value)}
            />
          </div>
        ) : null}
        {error ? (
          <p className="luna-research__error" role="alert">
            {error}
          </p>
        ) : null}
        <div className="luna-dialog__actions">
          <button ref={closeRef} type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
            {t("common:action.close")}
          </button>
          {topics ? (
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={busy !== null || text.trim() === ""}
              onClick={() => void save()}
            >
              {t("creator.research.saveTopic")}
            </button>
          ) : null}
        </div>
      </div>
    </div>,
    document.body,
  );
}
