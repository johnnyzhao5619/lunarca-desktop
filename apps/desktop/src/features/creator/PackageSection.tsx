/**
 * 发布包页的「〈渠道〉包装」一步（ADR-166 ④，PRD-19-022 / 023）：一个渠道一份。
 *
 * AI 按这个渠道的上限与风格写三个标题、简介正文、封面字、分类与标签；章节 / 参考来源 / 链接由
 * Rust 按固定顺序拼进简介。上限与检查都在 Rust（`creator_check_package`，只核对这个渠道有出处的
 * 上限）；写进发布笔记的「〈渠道〉包装」一节（`creator_save_package`），打开时从那一节填回
 * （`creator_package`）。五块都可直接改，这里只发命令、渲染结果（ADR-003）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type ChannelLimitsDto, type PackageDto, type PackageIssueDto, type PublishSourceDto } from "../../ipc/bindings";
import { noteStem } from "../../lib/note-path";
import { vaultErrorText } from "../../lib/vault-error";
import { ExtraRequestField, extraRequest } from "./ExtraRequestField";
import { packageIssueText } from "./publish-issues";

/** 停手多久再检查一次（毫秒）。 */
const CHECK_DELAY_MS = 250;

const EMPTY_PACKAGE: PackageDto = { titles: "", description: "", cover: "", category: "", tags: "" };

/**
 * 还没写进发布笔记的包装，按「项目 + 渠道」在进程内记住（切走再回来不丢；纯呈现层的选择记忆，
 * 与章节草稿同理）。
 */
const drafts = new Map<string, PackageDto>();

/** 五块可编辑文字的顺序与各自的行数。 */
const FIELDS = [
  ["titles", 3],
  ["description", 8],
  ["cover", 2],
  ["category", 1],
  ["tags", 4],
] as const;
type Field = (typeof FIELDS)[number][0];

/** 这个渠道有出处的上限，一条一句（没有的不写）。 */
const LIMITS = [
  ["title_max", "title"],
  ["body_max", "description"],
  ["tags_max", "tags"],
  ["tag_max", "tag"],
] as const satisfies readonly (readonly [keyof ChannelLimitsDto, string])[];

export interface PackageSectionProps {
  home: string;
  source: PublishSourceDto | null;
  /** 渠道 id。 */
  channel: string;
  /** 渠道给人看的名字。 */
  name: string;
  /** 这个渠道的上限；目录还没取回时为 `null`。 */
  limits: ChannelLimitsDto | null;
  /** 这个平台发布时要声明 AI 内容（发布须知，ADR-166）。 */
  aiNotice: boolean;
  onOpenNote: (path: string) => void;
  /** 写进发布笔记之后（清单重判这一步）。 */
  onSaved?: () => void;
}

export function PackageSection({ home, source, channel, name, limits, aiNotice, onOpenNote, onSaved }: PackageSectionProps) {
  const { t } = useTranslation("workspace");
  const [draft, setDraft] = useState<PackageDto>(EMPTY_PACKAGE);
  const [issues, setIssues] = useState<PackageIssueDto[] | null>(null);
  const [generating, setGenerating] = useState(false);
  const [extra, setExtra] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const checkRequestRef = useRef(0);
  const draftKey = `${home}\n${channel}`;
  const headingId = `publish-package-${channel}`;

  const updateDraft = (next: PackageDto) => {
    setDraft(next);
    setNotice(null);
    setSavedPath(null);
    drafts.set(draftKey, next);
  };

  // 有没写盘的草稿就接着改；否则从发布笔记里那一节填回。
  useEffect(() => {
    setIssues(null);
    setNotice(null);
    setSavedPath(null);
    const pending = drafts.get(draftKey);
    if (pending) {
      setDraft(pending);
      return;
    }
    setDraft(EMPTY_PACKAGE);
    let live = true;
    void commands.creatorPackage(home, channel).then((reply) => {
      if (live && reply.status === "ok" && reply.data && !drafts.has(draftKey)) setDraft(reply.data);
    });
    return () => {
      live = false;
    };
  }, [draftKey]);

  const hasText = FIELDS.some(([field]) => draft[field].trim().length > 0);

  // 边改边查：停手一拍再问 Rust；后到的旧结果不覆盖新结果。
  useEffect(() => {
    const request = ++checkRequestRef.current;
    if (!hasText) {
      setIssues(null);
      return;
    }
    const timer = window.setTimeout(() => {
      void commands.creatorCheckPackage(channel, draft).then((reply) => {
        if (request === checkRequestRef.current && reply.status === "ok") setIssues(reply.data);
      });
    }, CHECK_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [draft, hasText, channel]);

  const generate = async () => {
    if (!source) return;
    setGenerating(true);
    setNotice(null);
    const reply = await commands.creatorGeneratePackage(home, source.path, channel, extraRequest(extra));
    setGenerating(false);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    updateDraft(reply.data.package);
    setIssues(reply.data.issues);
  };

  const save = async () => {
    setNotice(null);
    const reply = await commands.creatorSavePackage(home, channel, draft);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    drafts.delete(draftKey);
    setSavedPath(reply.data);
    onSaved?.();
  };

  const copy = async (field: Field) => {
    try {
      await navigator.clipboard.writeText(draft[field]);
      setNotice(t("creator.publish.common.copied", { what: t(`creator.publish.package.${field}`) }));
    } catch {
      setNotice(t("creator.publish.common.copyFailed"));
    }
  };

  const known = limits
    ? LIMITS.flatMap(([key, label]) => {
        const max = limits[key];
        return max === null ? [] : [t(`creator.publish.package.limit.${label}`, { count: max })];
      })
    : [];

  return (
    <section className="luna-creator-page__section" aria-labelledby={headingId}>
      <h2 id={headingId}>{t("creator.publish.step.package", { channel: name })}</h2>
      <p className="luna-creator-page__hint">{t("creator.publish.package.hint")}</p>
      {limits ? (
        <p className="luna-creator-page__hint">
          {known.length > 0
            ? t("creator.publish.package.limits", { list: known.join(t("creator.publish.package.limitSeparator")) })
            : t("creator.publish.package.noLimits")}
        </p>
      ) : null}
      <ExtraRequestField value={extra} onChange={setExtra} />
      {aiNotice ? (
        <p className="luna-creator-page__hint" role="note">
          {t(`creator.channelNotice.${channel}`)}
        </p>
      ) : null}
      <div className="luna-creator-page__row">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={!source || generating}
          onClick={() => void generate()}
        >
          {t(generating ? "creator.publish.common.generating" : "creator.publish.package.generate")}
        </button>
        {hasText ? <span className="luna-creator-page__hint">{t("creator.publish.package.hintReplace")}</span> : null}
      </div>
      {FIELDS.map(([field, rows]) => (
        <div key={field} className="luna-creator-page__field">
          <div className="luna-creator-page__row">
            <span className="luna-creator-page__label">{t(`creator.publish.package.${field}`)}</span>
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              disabled={!draft[field].trim()}
              onClick={() => void copy(field)}
            >
              {t("creator.publish.common.copy")}
            </button>
          </div>
          <textarea
            className="luna-publish__chapters"
            rows={rows}
            value={draft[field]}
            placeholder={t(`creator.publish.package.${field}Placeholder`)}
            aria-label={t(`creator.publish.package.${field}`)}
            onChange={(event) => updateDraft({ ...draft, [field]: event.target.value })}
          />
        </div>
      ))}
      {issues && issues.length > 0 ? (
        <ul className="luna-publish__issues" aria-label={t("creator.publish.common.issues")}>
          {issues.map((issue, index) => (
            <li key={`${issue.kind}-${index}`}>{packageIssueText(t, issue)}</li>
          ))}
        </ul>
      ) : null}
      <div className="luna-creator-page__row">
        <button type="button" className="luna-btn luna-btn--primary luna-btn--sm" disabled={!hasText} onClick={() => void save()}>
          {t("creator.publish.common.save")}
        </button>
      </div>
      {savedPath ? (
        <div className="luna-creator-page__row" role="status">
          <span className="luna-creator-page__hint">
            {t("creator.publish.package.saved", {
              note: noteStem(savedPath),
              section: t("creator.publish.step.package", { channel: name }),
            })}
          </span>
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
