/**
 * 「添加来源」（ADR-167 ③④，PRD-19-044；对标 NotebookLM 的添加来源）：一个对话框、五种来源——
 * 文件（选择或拖进来；音视频转写、文档转换，与原「导入素材」同一条管线）、网页链接（剪藏）、
 * 视频链接（「从链接导入」应用启用后才有；第一次用前确认一次权利声明，下载转写后建进项目的素材
 * 文件夹）、粘贴文字（存成「素材」里的一篇）、库里已有的笔记（链进项目，不复制）。
 *
 * 状态与命令都在 `useMaterialImport`（页头主动作与概览共用一份）；建在哪、链进哪篇、能不能剪藏
 * 全由 Rust 判，这里只收集输入（ADR-003）。
 */

import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { commands, type SearchHitDto } from "../../ipc/bindings";
import { useAppEnabled } from "../../hooks/use-apps";
import { fetchErrorKey, unknownFetchErrorKey } from "../../lib/task-failure";
import { FETCH_MEDIA_CHOICES, useUrlImport, type FetchMediaChoice } from "../transcribe/use-url-import";
import { useLinkImportRights } from "../transcribe/use-link-import-rights";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { useTablistNavigation } from "../../hooks/use-tablist-navigation";
import { goTo } from "../../lib/navigate";
import { appDetailRoute } from "../../lib/app-route";
import { PROJECT_DROP_ATTR } from "../../lib/project-file-drop";
import type { MaterialImport } from "./use-material-import";
import { useProjectDrop } from "./use-project-drop";

/** 五种来源（顺序即标签顺序）；视频链接只在「从链接导入」应用启用后出现。 */
const KINDS = ["file", "web", "video", "text", "vault"] as const;
type SourceTab = (typeof KINDS)[number];

/** 「从链接导入」的组件在哪管（ADR-167 ⑤）。 */
const LINK_IMPORT_ROUTE = appDetailRoute("link_import");

/** 「从库里选」一次列多少篇（按修改时间，新的在前）。 */
const VAULT_PICK_LIMIT = 30;

/** 「网页剪藏」开关在设置里的位置（关着时从提示直达，PRD-09-087）。 */
const WEB_CLIP_SETTING_ROUTE = "settings://privacy/privacy.web_clip";

export interface AddSourcesDialogProps {
  home: string;
  materials: MaterialImport;
}

export function AddSourcesDialog({ home, materials }: AddSourcesDialogProps) {
  const { t } = useTranslation("workspace");
  const { t: tTranscribe } = useTranslation("transcribe");
  const linkImport = useAppEnabled("link_import");
  const kinds = KINDS.filter((kind) => kind !== "video" || linkImport);
  const [tab, setTab] = useState<SourceTab>("file");
  const [videoUrl, setVideoUrl] = useState("");
  const video = useUrlImport(home);
  const ensureRights = useLinkImportRights();
  const [title, setTitle] = useState("");
  const [text, setText] = useState("");
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHitDto[]>([]);
  const [picked, setPicked] = useState<string[]>([]);
  const firstRef = useRef<HTMLButtonElement | null>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose: materials.closeAdd, initialFocusRef: firstRef });
  const navigation = useTablistNavigation<SourceTab>(kinds, tab, setTab);
  const dropRef = useProjectDrop<HTMLDivElement>((paths) => void materials.importPaths(paths));

  // 「从库里选」：按标题 / 路径筛，新改的在前；项目主页自己不列。
  useEffect(() => {
    if (tab !== "vault") return;
    let live = true;
    const timer = window.setTimeout(() => {
      void commands
        .search({
          text: null,
          note_type: null,
          tag: null,
          path_prefix: null,
          name_like: query.trim() === "" ? null : query.trim(),
          limit: VAULT_PICK_LIMIT,
          page: { sort: "modified", cursor: null },
        })
        .then((reply) => {
          if (live && reply.status === "ok") setHits(reply.data.filter((hit) => hit.path !== home));
        });
    }, 150);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [tab, query, home]);

  const togglePicked = (path: string) =>
    setPicked((current) => (current.includes(path) ? current.filter((entry) => entry !== path) : [...current, path]));

  const panelId = (kind: SourceTab) => `add-sources-panel-${kind}`;
  const tabId = (kind: SourceTab) => `add-sources-tab-${kind}`;

  return createPortal(
    <div className="luna-overlay" role="presentation" onClick={materials.closeAdd}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-add-sources"
        role="dialog"
        aria-modal="true"
        aria-labelledby="add-sources-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="add-sources-title">{t("creator.addSources.title")}</h3>
          <p>{t("creator.addSources.description")}</p>
        </header>
        <div className="luna-add-sources__tabs" role="tablist" aria-label={t("creator.addSources.title")}>
          {kinds.map((kind, index) => (
            <button
              key={kind}
              ref={(element) => {
                navigation.register(kind)(element);
                if (index === 0) firstRef.current = element;
              }}
              id={tabId(kind)}
              type="button"
              role="tab"
              className="luna-add-sources__tab"
              aria-selected={tab === kind}
              aria-controls={panelId(kind)}
              tabIndex={navigation.tabIndexOf(kind)}
              onKeyDown={navigation.onKeyDown}
              onClick={() => setTab(kind)}
            >
              {t(`creator.addSources.tab.${kind}`)}
            </button>
          ))}
        </div>

        <div id={panelId(tab)} role="tabpanel" aria-labelledby={tabId(tab)} className="luna-add-sources__panel">
          {tab === "file" ? (
            <div ref={dropRef} className="luna-add-sources__drop" {...{ [PROJECT_DROP_ATTR]: home }}>
              <p>{t("creator.addSources.dropHint")}</p>
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={materials.importing}
                onClick={() => void materials.importMaterial()}
              >
                {t(materials.importing ? "creator.addSources.importing" : "creator.addSources.pickFiles")}
              </button>
              <p className="luna-creator-page__hint">{t("creator.addSources.fileTypes")}</p>
            </div>
          ) : null}

          {tab === "web" ? (
            <form
              className="luna-add-sources__form"
              noValidate
              onSubmit={(event) => {
                event.preventDefault();
                void materials.clipWeb();
              }}
            >
              <label className="luna-add-sources__field">
                <span>{t("creator.addSources.webLabel")}</span>
                <input
                  className="luna-input"
                  type="url"
                  inputMode="url"
                  placeholder={t("creator.addSources.webPlaceholder")}
                  value={materials.clipUrl}
                  onChange={(event) => materials.setClipUrl(event.target.value)}
                />
              </label>
              <p className="luna-creator-page__hint">{t("creator.addSources.webHint")}</p>
              <div className="luna-dialog__actions">
                <button
                  type="submit"
                  className="luna-btn luna-btn--primary"
                  disabled={materials.clipping || materials.clipUrl.trim() === ""}
                >
                  {t(materials.clipping ? "creator.addSources.clipping" : "creator.addSources.clip")}
                </button>
              </div>
            </form>
          ) : null}

          {tab === "video" ? (
            <div className="luna-add-sources__form">
              {video.toolchain.status !== null && !video.toolchain.status.ready ? (
                <p className="luna-creator-page__hint">
                  {t("creator.addSources.videoNeedsComponents")}{" "}
                  <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => goTo(LINK_IMPORT_ROUTE)}>
                    {t("creator.addSources.videoOpenApp")}
                  </button>
                </p>
              ) : null}
              <form
                className="luna-add-sources__form"
                noValidate
                onSubmit={(event) => {
                  event.preventDefault();
                  if (videoUrl.trim() !== "") void video.probeUrl(videoUrl);
                }}
              >
                <label className="luna-add-sources__field">
                  <span>{t("creator.addSources.videoLabel")}</span>
                  <input
                    className="luna-input"
                    type="url"
                    inputMode="url"
                    placeholder={t("creator.addSources.videoPlaceholder")}
                    value={videoUrl}
                    onChange={(event) => {
                      setVideoUrl(event.target.value);
                      if (video.phase === "confirm") video.reset();
                    }}
                  />
                </label>
                {video.phase !== "confirm" ? (
                  <div className="luna-dialog__actions">
                    <button
                      type="submit"
                      className="luna-btn luna-btn--secondary"
                      disabled={video.phase === "probing" || videoUrl.trim() === ""}
                    >
                      {t(video.phase === "probing" ? "creator.addSources.videoProbing" : "creator.addSources.videoProbe")}
                    </button>
                  </div>
                ) : null}
              </form>
              {video.phase === "confirm" && video.probe ? (
                <>
                  <ul className="luna-add-sources__hits" aria-label={t("creator.addSources.videoEntries")}>
                    {video.probe.entries.map((entry) => (
                      <li key={entry.url}>
                        <label className="luna-add-sources__hit" title={entry.url}>
                          <input
                            type="checkbox"
                            checked={video.selected.has(entry.url)}
                            disabled={entry.is_live}
                            onChange={() => video.toggle(entry.url)}
                          />
                          <span className="luna-add-sources__hit-title">{entry.title}</span>
                          <span className="luna-add-sources__hit-path">{video.probe?.site ?? ""}</span>
                        </label>
                      </li>
                    ))}
                  </ul>
                  <fieldset className="luna-add-sources__media">
                    <legend>{tTranscribe("fetch.mediaLegend")}</legend>
                    {FETCH_MEDIA_CHOICES.map((choice: FetchMediaChoice) => (
                      <label key={choice} className="luna-add-sources__choice">
                        <input
                          type="radio"
                          name="add-sources-media"
                          checked={video.media === choice}
                          onChange={() => video.setMedia(choice)}
                        />
                        <span>{tTranscribe(`fetch.media.${choice}`)}</span>
                      </label>
                    ))}
                  </fieldset>
                  <p className="luna-creator-page__hint">{tTranscribe("fetch.consent")}</p>
                  <div className="luna-dialog__actions">
                    <button
                      type="button"
                      className="luna-btn luna-btn--primary"
                      disabled={video.submitting || video.selected.size === 0}
                      onClick={() =>
                        void (async () => {
                          if (!(await ensureRights())) return;
                          const count = await video.submit();
                          if (count > 0) {
                            setVideoUrl("");
                            video.reset();
                            materials.linkQueued(count);
                          }
                        })()
                      }
                    >
                      {t("creator.addSources.videoAdd", { count: video.selected.size })}
                    </button>
                  </div>
                </>
              ) : null}
              {video.error !== null ? (
                <p className="luna-add-sources__notice" role="alert">
                  {tTranscribe(fetchErrorKey(video.error) ?? unknownFetchErrorKey)}
                </p>
              ) : null}
            </div>
          ) : null}

          {tab === "text" ? (
            <form
              className="luna-add-sources__form"
              noValidate
              onSubmit={(event) => {
                event.preventDefault();
                void materials.addText(title, text).then((ok) => {
                  if (!ok) return;
                  setTitle("");
                  setText("");
                });
              }}
            >
              <label className="luna-add-sources__field">
                <span>{t("creator.addSources.textTitle")}</span>
                <input
                  className="luna-input"
                  placeholder={t("creator.addSources.textTitlePlaceholder")}
                  value={title}
                  onChange={(event) => setTitle(event.target.value)}
                />
              </label>
              <label className="luna-add-sources__field">
                <span>{t("creator.addSources.textBody")}</span>
                <textarea
                  className="luna-add-sources__text"
                  rows={8}
                  placeholder={t("creator.addSources.textPlaceholder")}
                  value={text}
                  onChange={(event) => setText(event.target.value)}
                />
              </label>
              <div className="luna-dialog__actions">
                <button
                  type="submit"
                  className="luna-btn luna-btn--primary"
                  disabled={materials.saving || text.trim() === ""}
                >
                  {t("creator.addSources.addText")}
                </button>
              </div>
            </form>
          ) : null}

          {tab === "vault" ? (
            <div className="luna-add-sources__form">
              <label className="luna-add-sources__field">
                <span>{t("creator.addSources.vaultLabel")}</span>
                <input
                  className="luna-input"
                  type="search"
                  placeholder={t("creator.addSources.vaultPlaceholder")}
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </label>
              {hits.length === 0 ? (
                <p className="luna-creator-page__hint">{t("creator.addSources.vaultEmpty")}</p>
              ) : (
                <ul className="luna-add-sources__hits" aria-label={t("creator.addSources.vaultLabel")}>
                  {hits.map((hit) => (
                    <li key={hit.path}>
                      <label className="luna-add-sources__hit" title={hit.path}>
                        <input
                          type="checkbox"
                          checked={picked.includes(hit.path)}
                          onChange={() => togglePicked(hit.path)}
                        />
                        <span className="luna-add-sources__hit-title">{hit.title}</span>
                        <span className="luna-add-sources__hit-path">{hit.path}</span>
                      </label>
                    </li>
                  ))}
                </ul>
              )}
              <p className="luna-creator-page__hint">{t("creator.addSources.vaultHint")}</p>
              <div className="luna-dialog__actions">
                <button
                  type="button"
                  className="luna-btn luna-btn--primary"
                  disabled={materials.saving || picked.length === 0}
                  onClick={() =>
                    void materials.linkNotes(picked).then((ok) => {
                      if (ok) setPicked([]);
                    })
                  }
                >
                  {t("creator.addSources.link", { count: picked.length })}
                </button>
              </div>
            </div>
          ) : null}
        </div>

        {materials.notice ? (
          <p className="luna-add-sources__notice" role="status">
            {materials.notice}
            {materials.clipDisabled ? (
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                onClick={() => goTo(WEB_CLIP_SETTING_ROUTE)}
              >
                {t("creator.addSources.openClipSetting")}
              </button>
            ) : null}
          </p>
        ) : null}
        <div className="luna-dialog__actions">
          <button type="button" className="luna-btn luna-btn--secondary" onClick={materials.closeAdd}>
            {t("common:action.close")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
