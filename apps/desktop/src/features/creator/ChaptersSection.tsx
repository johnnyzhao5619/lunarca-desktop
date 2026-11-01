/**
 * 发布包页的「章节」一节（PRD-19-021，ADR-162 ⑪）。
 *
 * 章节就是 YouTube 简介里的那几行文字（`0:00 开场`），用户直接改文字。生成、解析、按
 * YouTube 规则校验与修正全在 Rust（`creator_generate_chapters` / `creator_check_chapters`）；
 * 这里只发命令、渲染问题与建议（ADR-003）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type ChapterIssueDto,
  type ChaptersCheckDto,
  type PublishSourceDto,
} from "../../ipc/bindings";
import { noteStem } from "../../lib/note-path";
import { vaultErrorText } from "../../lib/vault-error";
import { ExtraRequestField, extraRequest } from "./ExtraRequestField";
import { chapterIssueText } from "./publish-issues";

/** 停手多久再检查一次（毫秒）：边改边查，但不每个键都跑一趟。 */
const CHECK_DELAY_MS = 250;

/**
 * 还没写进发布笔记的章节文字，按「项目 + 成片」在进程内记住：切到别的空间再回来，页面会
 * 重新挂载，不该把用户改了一半的文字丢掉。纯呈现层的选择记忆，与导出选项同理。
 */
const drafts = new Map<string, string>();

export interface ChaptersSectionProps {
  /** 项目主页的库相对路径。 */
  home: string;
  /** 选中的成片；没有时不能生成。 */
  source: PublishSourceDto | null;
  /** 打开一篇笔记（写进发布笔记后「打开」）。 */
  onOpenNote: (path: string) => void;
  /** 写进发布笔记之后（清单重判这一步）。 */
  onSaved?: () => void;
}

export function ChaptersSection({ home, source, onOpenNote, onSaved }: ChaptersSectionProps) {
  const { t } = useTranslation("workspace");
  const [text, setText] = useState("");
  const [durationMs, setDurationMs] = useState<number | null>(null);
  const [check, setCheck] = useState<ChaptersCheckDto | null>(null);
  const [generating, setGenerating] = useState(false);
  const [extra, setExtra] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const checkRequestRef = useRef(0);
  const draftKey = source ? `${home}\n${source.path}` : null;

  /** 改文字（手打、生成、按建议修正）一律经这里：记住草稿，旧的提示与「已写进」都作废。 */
  const updateText = (next: string) => {
    setText(next);
    setNotice(null);
    setSavedPath(null);
    if (draftKey) drafts.set(draftKey, next);
  };

  // 换了成片：时长跟着换，文字换成这一篇记住的草稿（没有就空）。
  useEffect(() => {
    setDurationMs(source?.duration_secs != null ? source.duration_secs * 1000 : null);
    setText(draftKey ? (drafts.get(draftKey) ?? "") : "");
    setCheck(null);
    setNotice(null);
    setSavedPath(null);
  }, [draftKey, source?.duration_secs]);

  // 边改边查：停手一拍再问 Rust；后到的旧结果不覆盖新结果。
  useEffect(() => {
    const request = ++checkRequestRef.current;
    if (!text.trim()) {
      setCheck(null);
      return;
    }
    const timer = window.setTimeout(() => {
      void commands.creatorCheckChapters(text, durationMs).then((result) => {
        if (request === checkRequestRef.current) setCheck(result);
      });
    }, CHECK_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [text, durationMs]);

  const generate = async () => {
    if (!source) return;
    setGenerating(true);
    setNotice(null);
    setSavedPath(null);
    const reply = await commands.creatorGenerateChapters(home, source.path, extraRequest(extra));
    setGenerating(false);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    setDurationMs(reply.data.duration_ms);
    updateText(reply.data.text);
    setCheck(reply.data.check);
  };

  const save = async () => {
    setNotice(null);
    const reply = await commands.creatorSaveChapters(home, text);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    if (draftKey) drafts.delete(draftKey);
    setSavedPath(reply.data);
    onSaved?.();
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setNotice(t("creator.publish.chapters.copied"));
    } catch {
      setNotice(t("creator.publish.common.copyFailed"));
    }
  };

  const issueText = (issue: ChapterIssueDto) => chapterIssueText(t, issue);

  const hasText = text.trim().length > 0;

  return (
    <section className="luna-creator-page__section" aria-labelledby="publish-chapters">
      <h2 id="publish-chapters">{t("creator.publish.chapters.title")}</h2>
      <p className="luna-creator-page__hint">{t(hasText ? "creator.publish.chapters.hintReplace" : "creator.publish.chapters.hint")}</p>
      <ExtraRequestField value={extra} onChange={setExtra} />
      <div className="luna-creator-page__row">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={!source || generating}
          onClick={() => void generate()}
        >
          {t(generating ? "creator.publish.common.generating" : "creator.publish.chapters.generate")}
        </button>
      </div>
      <textarea
        className="luna-publish__chapters"
        rows={8}
        value={text}
        placeholder={t("creator.publish.chapters.placeholder")}
        aria-label={t("creator.publish.chapters.title")}
        onChange={(event) => updateText(event.target.value)}
      />
      {check && check.issues.length > 0 ? (
        <ul className="luna-publish__issues" aria-label={t("creator.publish.common.issues")}>
          {check.issues.map((issue, index) => (
            <li key={`${issue.kind}-${index}`}>{issueText(issue)}</li>
          ))}
        </ul>
      ) : null}
      {check && check.issues.length === 0 ? (
        <p className="luna-creator-page__hint" role="status">
          {t("creator.publish.chapters.valid")}
        </p>
      ) : null}
      <div className="luna-creator-page__row">
        {check?.fixed ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => {
              if (check.fixed) updateText(check.fixed);
            }}
          >
            {t("creator.publish.chapters.fix")}
          </button>
        ) : null}
        <button type="button" className="luna-btn luna-btn--primary luna-btn--sm" disabled={!hasText} onClick={() => void save()}>
          {t("creator.publish.common.save")}
        </button>
        <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" disabled={!hasText} onClick={() => void copy()}>
          {t("creator.publish.common.copy")}
        </button>
      </div>
      {savedPath ? (
        <div className="luna-creator-page__row" role="status">
          <span className="luna-creator-page__hint">{t("creator.publish.chapters.saved", { note: noteStem(savedPath) })}</span>
          <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => onOpenNote(savedPath)}>
            {t("creator.publish.common.open")}
          </button>
        </div>
      ) : null}
      {notice ? (
        <p className="luna-creator-page__hint" role="status">
          {notice}
        </p>
      ) : null}
    </section>
  );
}
