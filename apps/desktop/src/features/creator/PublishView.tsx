/**
 * 发布包页（ADR-162 ⑪，PRD-19-018~020）：一个项目的成片 → 字幕一次出齐。
 *
 * 分步清单（ADR-165 ⑧，PRD-19-039）：成片 · 字幕 · 章节 · 每个发布渠道一步包装（ADR-166 ④）· 检查，
 * 左侧步骤列标出每步完成与否、页头写完成几步、发到哪些渠道。步骤裁剪、完成度、前置与「检查」的汇总
 * 全由 Rust 判定（`creator_publish_checklist`）；各步的表单一直挂着（切步不丢草稿），只显示当前
 * 那一步。
 *
 * 成片候选、写哪几份、叫什么名字、简繁转换与格式渲染全在 Rust（`creator_publish_sources` /
 * `creator_export_subtitles`）；章节一节见 `ChaptersSection`，渠道包装见 `PackageSection`；这里只发命令、渲染结果
 * （ADR-003）。没有译文时给「翻译」，走
 * 既有的转译队列，翻好后边车与元数据变化经 vault 变更刷新这一页。
 */

import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  events,
  type HanVariantDto,
  type PublishCheckDto,
  type PublishChecklistDto,
  type PublishSourceDto,
  type PublishStepDto,
  type PublishSubtitleFormat,
  type PublishSubtitlesDto,
} from "../../ipc/bindings";
import { EmptyState } from "../../components/ui/EmptyState";
import { CheckIcon, FilmIcon } from "../../components/ui/Icon";
import { SelectBox } from "../../components/ui/SelectBox";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { formatClock } from "../../lib/format-duration";
import { vaultErrorText } from "../../lib/vault-error";
import { TRANSLATE_TARGET_SETTING, useSettingOptions } from "../settings/language-options";
import { HAN_VARIANTS } from "../transcribe/export-options";
import { useTranscribeImport } from "../transcribe/use-transcribe-import";
import { ChaptersSection } from "./ChaptersSection";
import { ChannelPicker } from "./ChannelPicker";
import { PageLoading } from "./CreatorPage";
import { PackageSection } from "./PackageSection";
import { chapterIssueText, packageIssueText } from "./publish-issues";
import { channelNameById, useChannels } from "./use-channels";

/** 「检查」里的一条属于哪一步（点它回到那一步）。 */
function stepOfCheck(check: PublishCheckDto): PublishStepDto {
  switch (check.kind) {
    case "missing":
      return check.step;
    case "chapters":
      return { kind: "chapters" };
    case "package":
      return { kind: "package", channel: check.channel };
  }
}

/** 一步的身份（包装按渠道各算一步）：比较、React key 与元素 id 都用它。 */
function stepKey(step: PublishStepDto): string {
  return step.kind === "package" ? `package-${step.channel}` : step.kind;
}

/** 发布包字幕的主格式（BCC 是另勾的一份）。 */
const SUBTITLE_FORMATS: readonly PublishSubtitleFormat[] = ["srt", "vtt"];

export interface PublishViewProps {
  /** 项目主页的库相对路径。 */
  home: string;
  /** 成片已排进导入队列（打开处理队列看进度）。 */
  onImportQueued: () => void;
  /** 打开一篇笔记（发布笔记）。 */
  onOpenNote: (path: string) => void;
}

export function PublishView({ home, onImportQueued, onOpenNote }: PublishViewProps) {
  const { t } = useTranslation("workspace");
  const [sources, setSources] = useState<PublishSourceDto[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [format, setFormat] = useState<PublishSubtitleFormat>("srt");
  const [han, setHan] = useState<HanVariantDto>("keep");
  const [bcc, setBcc] = useState(false);
  const [translateTarget, setTranslateTarget] = useState("smart");
  const translateTargets = useSettingOptions(TRANSLATE_TARGET_SETTING);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [result, setResult] = useState<PublishSubtitlesDto | null>(null);
  const [importNotice, setImportNotice] = useState<string | null>(null);
  // 选文件、预估、入队与「转写」页同一条；只是转写笔记由 Rust 建进这个项目的文件夹。
  const importer = useTranscribeImport(
    (message) => setImportNotice(vaultErrorText(message)),
    (paths) => commands.creatorImportFinalCut(home, paths),
  );
  /** 最近一次取数（后到的旧结果不覆盖新结果）。 */
  const requestRef = useRef(0);
  const [checklist, setChecklist] = useState<PublishChecklistDto | null>(null);
  const [step, setStep] = useState<PublishStepDto | null>(null);
  const catalog = useChannels();
  const checklistRequestRef = useRef(0);

  const loadChecklist = () => {
    const request = ++checklistRequestRef.current;
    void commands.creatorPublishChecklist(home).then((reply) => {
      if (request !== checklistRequestRef.current || reply.status !== "ok") return;
      setChecklist(reply.data);
      // 第一次进来停在第一个没做完的那一步；之后不替用户换步——除非那一步随渠道被去掉了。
      setStep((current) =>
        current !== null && reply.data.steps.some((item) => stepKey(item.step) === stepKey(current))
          ? current
          : ((reply.data.steps.find((item) => !item.done) ?? reply.data.steps[0])?.step ?? null),
      );
    });
  };
  useEffect(loadChecklist, [home]);
  useVaultReload(loadChecklist);
  // 在编辑器里改了发布笔记（或研究、脚本）：重判一次。
  const folder = home.includes("/") ? `${home.slice(0, home.lastIndexOf("/"))}/` : "";
  useEffect(() => {
    const unlisten = events.noteWritten.listen((event) => {
      if (event.payload.path.startsWith(folder)) loadChecklist();
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
    // 只随项目换；loadChecklist 每次渲染都是新的，但读的都是 ref 与 home。
  }, [folder]);

  const load = () => {
    loadChecklist();
    const request = ++requestRef.current;
    void commands.creatorPublishSources(home).then((reply) => {
      if (request !== requestRef.current) return;
      if (reply.status !== "ok") {
        setSources([]);
        setNotice(vaultErrorText(reply.error));
        return;
      }
      setSources(reply.data);
      setSelected((current) =>
        current && reply.data.some((source) => source.path === current) ? current : (reply.data[0]?.path ?? null),
      );
    });
  };

  useEffect(load, [home]);
  // 翻译跑完、成片被移进或移出项目文件夹：元数据变了，重取一次候选。
  useVaultReload(load);

  const channels = checklist?.channels ?? [];

  const source = sources?.find((item) => item.path === selected) ?? null;

  const importFinalCut = async () => {
    setImportNotice(null);
    const queued = await importer.start();
    if (queued > 0) {
      setImportNotice(t("creator.publish.importQueued", { count: queued }));
      onImportQueued();
    }
  };

  const importButton = (
    <button
      type="button"
      className="luna-btn luna-btn--secondary luna-btn--sm"
      disabled={importer.importing}
      onClick={() => void importFinalCut()}
    >
      {t(importer.importing ? "creator.publish.importing" : "creator.publish.importFinalCut")}
    </button>
  );

  const translate = async () => {
    if (!source) return;
    setNotice(null);
    const reply = await commands.enqueueTranslate(source.path, translateTarget);
    setNotice(reply.status === "ok" ? t("creator.publish.translateQueued") : vaultErrorText(reply.error));
  };

  const exportSubtitles = async () => {
    if (!source) return;
    setBusy(true);
    setNotice(null);
    setResult(null);
    const reply = await commands.creatorExportSubtitles(home, source.path, format, bcc, han);
    setBusy(false);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    setResult(reply.data);
    loadChecklist();
  };

  const isCurrent = (value: PublishStepDto) => step !== null && stepKey(step) === stepKey(value);
  const stepLabel = (value: PublishStepDto) =>
    value.kind === "package"
      ? t("creator.publish.step.package", { channel: channelNameById(value.channel, catalog, t) })
      : t(`creator.publish.step.${value.kind}`);
  const checkText = (check: PublishCheckDto) => {
    switch (check.kind) {
      case "missing":
        return t("creator.publish.check.missing", { step: stepLabel(check.step) });
      case "chapters":
        return t("creator.publish.check.issue", { step: stepLabel(stepOfCheck(check)), text: chapterIssueText(t, check.issue) });
      case "package":
        return t("creator.publish.check.issue", { step: stepLabel(stepOfCheck(check)), text: packageIssueText(t, check.issue) });
    }
  };

  /** 一步的那一屏：前置不满足时先写明缺什么、去哪补，表单照常显示（能看不能跑）。 */
  const pane = (value: PublishStepDto, children: ReactNode) => {
    const state = checklist?.steps.find((item) => stepKey(item.step) === stepKey(value));
    if (!state) return null;
    return (
      <div
        key={stepKey(value)}
        className="luna-publish__pane"
        id={`publish-step-${stepKey(value)}`}
        hidden={!isCurrent(value)}
      >
        {state.blocked_by ? (
          <div className="luna-publish__blocked" role="note">
            <span>{t("creator.publish.check.blocked", { step: stepLabel(state.blocked_by) })}</span>
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => setStep(state.blocked_by)}
            >
              {t("creator.publish.check.goTo", { step: stepLabel(state.blocked_by) })}
            </button>
          </div>
        ) : null}
        {children}
      </div>
    );
  };

  return (
    // 项目工作区的「发布」页签（ADR-165 ⑥）：页头、面包屑与返回都在工作区，这里只有内容。
    <section className="luna-creator" aria-label={t("creator.publish.title")}>
      <div className="luna-publish__head">
        <p className="luna-publish__progress" role="status">
          {checklist ? t("creator.publish.check.progress", { done: checklist.done, total: checklist.total }) : null}
        </p>
        {checklist ? <ChannelPicker home={home} channels={channels} onChanged={loadChecklist} /> : null}
      </div>
      <div className="luna-publish__layout">
        <nav className="luna-publish__steps" aria-label={t("creator.publish.stepsLabel")}>
          <ol>
            {checklist?.steps.map((item, index) => (
              <li key={stepKey(item.step)}>
                <button
                  type="button"
                  className="luna-publish__step"
                  data-done={item.done || undefined}
                  aria-current={isCurrent(item.step) ? "step" : undefined}
                  aria-controls={`publish-step-${stepKey(item.step)}`}
                  onClick={() => setStep(item.step)}
                >
                  <span className="luna-publish__step-mark" aria-hidden="true">
                    {item.done ? <CheckIcon size={12} /> : index + 1}
                  </span>
                  <span className="luna-publish__step-name">{stepLabel(item.step)}</span>
                  <span className="luna-publish__step-state">
                    {item.done
                      ? item.issues > 0
                        ? t("creator.publish.check.issues", { count: item.issues })
                        : t("creator.publish.check.done")
                      : item.blocked_by
                        ? t("creator.publish.check.needs", { step: stepLabel(item.blocked_by) })
                        : item.step.kind === "check"
                          ? t("creator.publish.check.issues", { count: item.issues })
                          : t("creator.publish.check.todo")}
                  </span>
                </button>
              </li>
            ))}
          </ol>
        </nav>
      <div className="luna-creator-page__body">
        {pane({ kind: "final_cut" }, <section className="luna-creator-page__section" aria-labelledby="publish-sources">
          <h2 id="publish-sources">{t("creator.publish.sources")}</h2>
          {sources === null ? (
            <PageLoading />
          ) : sources.length === 0 ? (
            <EmptyState
              icon={<FilmIcon size={24} />}
              title={t("creator.publish.noSources")}
              description={t("creator.publish.noSourcesHint")}
              action={importButton}
            />
          ) : (
            <div className="luna-publish__sources" role="radiogroup" aria-label={t("creator.publish.sources")}>
              {sources.map((item) => (
                <button
                  key={item.path}
                  type="button"
                  role="radio"
                  aria-checked={item.path === selected}
                  className="luna-publish__source"
                  data-active={item.path === selected || undefined}
                  onClick={() => {
                    setSelected(item.path);
                    setResult(null);
                  }}
                >
                  <span className="luna-publish__source-title">{item.title}</span>
                  <span className="luna-publish__source-meta">
                    {item.duration_secs !== null ? formatClock(item.duration_secs * 1000) : null}
                    <span>{t(item.has_translation ? "creator.publish.hasTranslation" : "creator.publish.noTranslation")}</span>
                  </span>
                </button>
              ))}
            </div>
          )}
          {sources?.length ? <div className="luna-creator-page__row">{importButton}</div> : null}
          {importNotice ? (
            <p className="luna-creator-page__hint" role="status">
              {importNotice}
            </p>
          ) : null}
          {source && !source.has_translation ? (
            <div className="luna-creator-page__row">
              <span className="luna-creator-page__hint">{t("creator.publish.translateHint")}</span>
              <SelectBox
                size="sm"
                value={translateTarget}
                ariaLabel={t("creator.publish.translateTarget")}
                onChange={(event) => setTranslateTarget(event.currentTarget.value)}
              >
                <option value="smart">{t("playback:translate.smart")}</option>
                {translateTargets.map((lang) => (
                  <option key={lang} value={lang}>
                    {t(`common:lang.${lang}`)}
                  </option>
                ))}
              </SelectBox>
              <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => void translate()}>
                {t("creator.publish.translate")}
              </button>
            </div>
          ) : null}
        </section>)}

        {pane({ kind: "subtitles" }, <section className="luna-creator-page__section" aria-labelledby="publish-subtitles">
          <h2 id="publish-subtitles">{t("creator.publish.subtitles")}</h2>
          <p className="luna-creator-page__hint">
            {t(source?.has_translation === false ? "creator.publish.subtitlesSourceOnly" : "creator.publish.subtitlesHint")}
          </p>
          <div className="luna-creator-page__row">
            <span className="luna-creator-page__label">{t("creator.publish.format")}</span>
            <div className="luna-segmented-control" role="radiogroup" aria-label={t("creator.publish.format")}>
              {SUBTITLE_FORMATS.map((value) => (
                <button
                  key={value}
                  type="button"
                  role="radio"
                  aria-checked={format === value}
                  className="luna-segmented-control__item"
                  data-active={format === value}
                  onClick={() => setFormat(value)}
                >
                  {t(`transcribe:format.${value}`)}
                </button>
              ))}
            </div>
          </div>
          <div className="luna-creator-page__row">
            <span className="luna-creator-page__label">{t("transcribe:exportOptions.han")}</span>
            <div className="luna-segmented-control" role="radiogroup" aria-label={t("transcribe:exportOptions.han")}>
              {HAN_VARIANTS.map((value) => (
                <button
                  key={value}
                  type="button"
                  role="radio"
                  aria-checked={han === value}
                  className="luna-segmented-control__item"
                  data-active={han === value}
                  onClick={() => setHan(value)}
                >
                  {t(`transcribe:exportOptions.han_${value}`)}
                </button>
              ))}
            </div>
          </div>
          <label className="luna-creator-page__row">
            <input type="checkbox" checked={bcc} onChange={(event) => setBcc(event.target.checked)} />
            <span>{t("creator.publish.bcc")}</span>
          </label>
          <div className="luna-creator-page__row">
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={!source || busy}
              onClick={() => void exportSubtitles()}
            >
              {t(busy ? "creator.publish.exporting" : "creator.publish.export")}
            </button>
          </div>
          {result ? (
            <div className="luna-publish__result" role="status">
              <p>{t("creator.publish.written", { folder: result.folder, count: result.files.length })}</p>
              <ul>
                {result.files.map((file) => (
                  <li key={file}>{file.split("/").pop()}</li>
                ))}
              </ul>
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                onClick={() => void commands.revealVaultPath(result.folder)}
              >
                {t("common:menu.revealInFileManager")}
              </button>
            </div>
          ) : null}
          {notice ? (
            <p className="luna-creator-page__hint" role="status">
              {notice}
            </p>
          ) : null}
        </section>)}

        {pane({ kind: "chapters" }, <ChaptersSection home={home} source={source} onOpenNote={onOpenNote} onSaved={loadChecklist} />)}

        {checklist?.steps.map(({ step: value }) =>
          value.kind === "package"
            ? pane(
                value,
                <PackageSection
                  home={home}
                  source={source}
                  channel={value.channel}
                  name={channelNameById(value.channel, catalog, t)}
                  limits={catalog.find((entry) => entry.id === value.channel)?.limits ?? null}
                  aiNotice={catalog.find((entry) => entry.id === value.channel)?.ai_notice ?? false}
                  onOpenNote={onOpenNote}
                  onSaved={loadChecklist}
                />,
              )
            : null,
        )}

        {pane(
          { kind: "check" },
          <section className="luna-creator-page__section" aria-labelledby="publish-check">
            <h2 id="publish-check">{t("creator.publish.step.check")}</h2>
            {checklist && checklist.checks.length === 0 ? (
              <p className="luna-creator-page__hint">{t("creator.publish.check.allClear")}</p>
            ) : (
              <ul className="luna-publish__checks">
                {checklist?.checks.map((check, index) => (
                  <li key={`${check.kind}-${index}`}>
                    <span>{checkText(check)}</span>
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary luna-btn--sm"
                      onClick={() => setStep(stepOfCheck(check))}
                    >
                      {t("creator.publish.check.goTo", { step: stepLabel(stepOfCheck(check)) })}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>,
        )}
      </div>
      </div>
    </section>
  );
}
