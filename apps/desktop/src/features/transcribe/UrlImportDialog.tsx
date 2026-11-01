/**
 * 「从链接导入」对话框（PRD-05-006，MacWhisper / Descript 对标）。
 *
 * 流程刻意是**两步**而不是"粘上就下"：粘错一条链接、那是个 3 小时的直播、或那是个 200 集的
 * 播放列表——用户都该在花掉带宽与磁盘**之前**看见标题与时长。故 `idle → probing → confirm`，
 * 确认卡上一并给出「仅音频 / 视频」这个真正影响体积的选择（就地可切，不必去设置页）。
 *
 * 工具缺失不是错误弹窗而是**安装计划**：这台机器将装的几件、来源、约多大、装到哪（Rust 给，
 * 与「外部工具」面板同一份），主按钮执行它；自行安装的命令收在折叠区。严格本地模式则给
 * 「去关闭」而非「去安装」——两种处境的下一步动作不同。
 *
 * ADR-003 前端零业务：这里没有一行 URL 解析、站点判断或直播判定，全部读后端探测结果。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { useInstallGuides } from "../../hooks/use-install-guides";
import { InstallGuide } from "../../components/InstallGuide";
import { FetchInstallPlan } from "../../components/FetchInstallPlan";
import { HelpDetails } from "../../components/ui/HelpDetails";
import { commands, type UrlProbeEntryDto } from "../../ipc/bindings";
import {
  CheckSquareIcon,
  CloseIcon,
  DownloadIcon,
  FilmIcon,
  GlobeIcon,
  LinkIcon,
  MusicIcon,
  SettingsIcon,
  SquareIcon,
} from "../../components/ui/Icon";
import { formatBytes } from "../../lib/format-bytes";
import { formatDuration } from "../../lib/format-duration";
import { fetchErrorKey, unknownFetchErrorKey } from "../../lib/task-failure";
import { FETCH_MEDIA_CHOICES, useUrlImport, type FetchMediaChoice } from "./use-url-import";
import { isImeKeyEvent } from "../../lib/ime-key-event";
import { goTo } from "../../lib/navigate";
import { appDetailRoute } from "../../lib/app-route";
import { useLinkImportRights } from "./use-link-import-rights";

/**
 * 「就地更新 yt-dlp 有望修好」的错误码。
 *
 * `FETCH_HOST_REJECTED` 是 2026-08-23 真机缺陷的那一种：元信息取得到、下载被站点 403 拒——
 * 受管安装把版本冻结在安装那天，而站点的反爬策略每周都在变。
 */
const UPDATE_FIXES = ["FETCH_TOOL_OUTDATED", "FETCH_HOST_REJECTED"] as const;

/** 档位 → 图标（文案走 i18n，图标是呈现，故留在渲染层）。 */
const MEDIA_ICONS: Record<FetchMediaChoice, typeof MusicIcon> = {
  audio: MusicIcon,
  video: FilmIcon,
  link_only: LinkIcon,
};

export interface UrlImportDialogProps {
  onClose: () => void;
  /** 入队成功（件数 > 0）：父级据此打开队列面板并提示。 */
  onQueued: (count: number) => void;
  /** 跳转设置深链（`settings://<页>[/<key>]`）：严格本地去隐私页，工具与代理去外部工具页。 */
  onOpenSettings: (route: string) => void;
}

/** 严格本地开关的深链（关掉它才能联网下载）。 */
const STRICT_LOCAL_ROUTE = "settings://privacy/privacy.strict_local";
/** yt-dlp 路径、代理与 ffmpeg 都在「外部工具」页（Rust `setting_page`）。 */
/** 组件（yt-dlp、JS 运行时、ffmpeg）的管理在「应用 ▸ 从链接导入」（ADR-167 ⑤）。 */
const COMPONENTS_ROUTE = appDetailRoute("link_import");

/** 读取视频信息超过这么久仍无结果，就如实说"还在等站点"——不编百分比。 */
const PROBE_SLOW_MS = 10_000;

export function UrlImportDialog({ onClose, onQueued, onOpenSettings }: UrlImportDialogProps) {
  const { t, i18n } = useTranslation("transcribe");
  const { t: tCommon } = useTranslation("common");
  const importer = useUrlImport();
  const ensureRights = useLinkImportRights();
  const toolchain = importer.toolchain;
  const [draft, setDraft] = useState("");
  /** 本次探测已等得够久（只是计时，不是进度）。 */
  const [probeSlow, setProbeSlow] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  /** 本机平台的安装引导（命令随平台变，故由后端给——见 `useInstallGuides`）。 */
  const guideFor = useInstallGuides();

  // 打开即把剪贴板里的地址预填（用户刚从浏览器复制过来，这是最可能的下一步）。
  // 只在**确实是 http(s) 地址**时才填——把任意剪贴板内容塞进输入框是惊吓不是便利。
  useEffect(() => {
    let active = true;
    void commands.readClipboardText().then((result) => {
      if (!active || result.status !== "ok") {
        return;
      }
      const text = (result.data ?? "").trim();
      if (/^https?:\/\/\S+$/i.test(text)) {
        setDraft(text);
      }
    });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    inputRef.current?.focus();
  }, [importer.phase]);

  useEffect(() => {
    if (importer.phase !== "probing") {
      setProbeSlow(false);
      return;
    }
    const timer = window.setTimeout(() => setProbeSlow(true), PROBE_SLOW_MS);
    return () => window.clearTimeout(timer);
  }, [importer.phase]);

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有手写的 Esc。
  // 初始焦点仍落在 URL 输入框上（`inputRef`）——打开这张卡就是为了粘一条链接。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, initialFocusRef: inputRef });

  /**
   * 有在途操作（探测视频信息 / 安装工具 / 检测工具 / 入队）。
   *
   * 忙时**点遮罩不关闭**：读一条视频信息要好几秒，期间点到对话框旁边一下就前功尽弃、
   * 得从头再来（真机反馈）。而点遮罩多半是**误触**——真想走人的用户会去点 ✕ 或按 Esc，
   * 那两条是明确意图，故始终可关。区分"明确意图"与"可能误触"，而不是一刀切禁掉关闭。
   */
  const busy =
    importer.phase === "probing" ||
    toolchain.installing !== null ||
    toolchain.checking ||
    importer.submitting;

  const selectableCount = useMemo(
    () => (importer.probe?.entries ?? []).filter((entry) => !entry.is_live).length,
    [importer.probe],
  );

  async function startProbe() {
    if (draft.trim().length === 0 || importer.phase === "probing") {
      return;
    }
    await importer.probeUrl(draft);
  }

  async function startFetch() {
    // 第一次用之前确认一次权利声明（ADR-167 ④）。
    if (!(await ensureRights())) return;
    const count = await importer.submit();
    if (count > 0) {
      onQueued(count);
      onClose();
    }
  }

  function renderInstallProgress() {
    const installing = toolchain.installing;
    if (installing === null) {
      return null;
    }
    return (
      <p className="luna-url-import__status" role="status">
        <span className="luna-proc-progress__spinner" aria-hidden="true" />
        {installing.progress === ""
          ? t("fetch.installStarting")
          : t("fetch.installProgress", { name: installing.name, progress: installing.progress })}
      </p>
    );
  }

  function renderToolGate() {
    const tool = toolchain.status;
    if (!tool) {
      return <p className="luna-url-import__status">{t("fetch.checkingTool")}</p>;
    }
    // 严格本地模式：抓取是明确的联网行为，被硬拦截（AGENTS.md §7）。给「去设置」而非「去安装」。
    if (tool.strict_local) {
      return (
        <div className="luna-url-import__gate">
          <GlobeIcon size={28} className="luna-url-import__gate-icon" />
          <h3>{t("fetch.strictLocalTitle")}</h3>
          <p>{t("fetch.strictLocalBody")}</p>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            onClick={() => onOpenSettings(STRICT_LOCAL_ROUTE)}
          >
            <SettingsIcon size={14} />
            <span>{t("fetch.openPrivacySettings")}</span>
          </button>
        </div>
      );
    }
    if (!tool.ready) {
      const installing = toolchain.installing !== null;
      const busy = installing || toolchain.checking;
      /**
       * 四种处境，下一步动作各不相同——初版把后三种糊成一句"可能是权限不足或文件损坏"，
       * 于是**一份完全可用的工具**（探测超时而已）也被指引去"重装"（真机反馈）。
       * - `missing`：没装 → 按计划安装
       * - `timeout`：探测超时（工具很可能是好的，只是启动慢）→ **再检测一次**，不是重装
       * - `broken`：跑了但失败 → 重新安装覆盖它；找到的路径进「诊断」
       * - 其余（正在探测）→ 只说"正在检测"，不给任何结论
       */
      const state: "missing" | "timeout" | "broken" | "checking" = toolchain.checking
        ? "checking"
        : tool.tool_path === null
          ? "missing"
          : tool.probe_timed_out
            ? "timeout"
            : "broken";
      // 已经有一份受管安装时，装的语义是"覆盖重装"而不是"首次安装"——按钮得这么说。
      const installLabel = tool.managed_version === null ? "installNow" : "reinstall";
      const installButton = (primary: boolean) => (
        <button
          type="button"
          className={primary ? "luna-btn luna-btn--primary" : "luna-btn luna-btn--secondary"}
          disabled={busy || toolchain.plan === null}
          onClick={() => void toolchain.install()}
        >
          <DownloadIcon size={14} />
          <span>{installing ? t("fetch.installing") : t(`fetch.${installLabel}`)}</span>
        </button>
      );
      const recheckButton = (primary: boolean) => (
        <button
          type="button"
          className={primary ? "luna-btn luna-btn--primary" : "luna-btn luna-btn--secondary"}
          disabled={busy}
          onClick={() => void toolchain.refresh(true)}
        >
          {toolchain.checking ? t("fetch.checkingTool") : t("fetch.recheck")}
        </button>
      );
      return (
        <div className="luna-url-import__gate">
          <DownloadIcon size={28} className="luna-url-import__gate-icon" />
          <h3>{t(`fetch.state.${state}Title`)}</h3>
          <p>{t(`fetch.state.${state}Body`)}</p>

          {/* 安装前的计划只在"要装"的两种处境出现：超时时工具多半是好的，计划只会误导人去重装。 */}
          {(state === "missing" || state === "broken") && <FetchInstallPlan plan={toolchain.plan} />}

          {/* 主动作随处境变：超时时该「重新检测」，其余才是「安装」。 */}
          <div className="luna-url-import__gate-actions">
            {state === "timeout" ? (
              <>
                {recheckButton(true)}
                {installButton(false)}
              </>
            ) : (
              <>
                {installButton(true)}
                {recheckButton(false)}
              </>
            )}
          </div>
          {renderInstallProgress()}
          {toolchain.installError !== null && (
            <p className="luna-url-import__error" role="alert">
              {t(fetchErrorKey(toolchain.installError) ?? unknownFetchErrorKey)}
            </p>
          )}

          <div className="luna-url-import__manual">
          <HelpDetails summary={t(state === "broken" ? "fetch.diagnostics" : "fetch.manualInstall")}>
            {tool.tool_path && (
              <p className="luna-url-import__hint" title={tool.tool_path}>
                {t("fetch.foundAt", { path: tool.tool_path })}
              </p>
            )}
            {tool.managed_version !== null && (
              <p className="luna-url-import__hint">
                {t("fetch.managedInstalled", { version: tool.managed_version })}
              </p>
            )}
            {/* 引导块与「设置 · 外部工具」面板共用同一个原语：命令由后端按平台给。 */}
            <InstallGuide guide={guideFor?.("yt_dlp") ?? null} />
            <p className="luna-url-import__hint">{t("fetch.installHint")}</p>
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => goTo(COMPONENTS_ROUTE)}
            >
              <SettingsIcon size={14} />
              <span>{t("fetch.setToolPath")}</span>
            </button>
          </HelpDetails>
          </div>
        </div>
      );
    }
    return null;
  }

  /**
   * 合并安装的结论：yt-dlp 装好了（门已打开），但运行时那一半可能没成——不能沉默，
   * 用户点了一颗按钮，得知道拿到的是一件还是两件。
   */
  function renderInstallOutcome() {
    const outcome = toolchain.outcome;
    if (outcome === null) {
      return null;
    }
    if (outcome.js_runtime_error !== null) {
      return (
        <p className="luna-url-import__notice" role="status">
          <span>
            {t("fetch.installPartial", {
              version: outcome.tool_version,
              reason: t(fetchErrorKey(outcome.js_runtime_error) ?? unknownFetchErrorKey),
            })}
          </span>
          <button
            type="button"
            className="luna-url-import__notice-action"
            onClick={() => goTo(COMPONENTS_ROUTE)}
          >
            {t("fetch.openComponents")}
          </button>
        </p>
      );
    }
    return (
      <p className="luna-url-import__hint" role="status">
        {outcome.js_runtime_version === null
          ? t("fetch.installDone", { version: outcome.tool_version })
          : t("fetch.installDoneWithRuntime", {
              version: outcome.tool_version,
              runtime: outcome.js_runtime_version,
            })}
      </p>
    );
  }

  function renderEntry(entry: UrlProbeEntryDto) {
    const checked = importer.selected.has(entry.url);
    const meta = [
      entry.duration_ms === null ? null : formatDuration(entry.duration_ms / 1000),
      entry.filesize === null ? null : formatBytes(entry.filesize, i18n.language),
      entry.uploader,
    ].filter((part): part is string => Boolean(part));
    return (
      <li key={entry.id} className="luna-url-import__entry" data-live={entry.is_live}>
        <label className="luna-url-import__entry-label">
          <input
            type="checkbox"
            checked={checked && !entry.is_live}
            disabled={entry.is_live}
            onChange={() => importer.toggle(entry.url)}
          />
          <span className="luna-url-import__entry-text">
            <strong title={entry.title}>{entry.title}</strong>
            <small>
              {entry.is_live
                ? t("fetch.liveNotSupported")
                : meta.join(" · ") || t("fetch.unknownMeta")}
            </small>
          </span>
        </label>
      </li>
    );
  }

  function renderConfirm() {
    const probe = importer.probe;
    if (!probe) {
      return null;
    }
    // 「单卡 or 可勾选清单」的判据由 Rust 下发（ADR-003）：单条目的播放列表（`?list=` 单集）
    // 同样要按列表呈现，而"条目数 == 1"这个近似判据恰恰把它误判成单视频。
    const single = !probe.is_playlist;
    const allSelected = importer.selected.size >= selectableCount && selectableCount > 0;
    return (
      <div className="luna-url-import__confirm">
        <div className="luna-url-import__source">
          <GlobeIcon size={13} />
          <span>{probe.playlist_title ?? t("fetch.fromSite", { site: probe.site })}</span>
          {probe.is_playlist && (
            <span className="luna-url-import__source-count">
              {t("fetch.entryCount", { count: probe.entries.length })}
            </span>
          )}
        </div>

        {probe.truncated > 0 && (
          <p className="luna-url-import__notice">
            {t("fetch.truncated", { count: probe.truncated, limit: probe.limit })}
          </p>
        )}

        {!single && selectableCount > 1 && (
          <button
            type="button"
            className="luna-url-import__select-all"
            onClick={importer.toggleAll}
          >
            {allSelected ? <CheckSquareIcon size={14} /> : <SquareIcon size={14} />}
            <span>{allSelected ? t("fetch.clearSelection") : t("fetch.selectAll")}</span>
          </button>
        )}

        <ul className="luna-url-import__entries" data-single={single}>
          {probe.entries.map(renderEntry)}
        </ul>

        <fieldset className="luna-url-import__media">
          <legend>{t("fetch.mediaLegend")}</legend>
          {/* 数据驱动三档：档位是 Rust 的 `FetchMediaKind`，这里只映射图标与文案键。 */}
          <div className="luna-url-import__media-options">
            {FETCH_MEDIA_CHOICES.map((choice) => {
              const Icon = MEDIA_ICONS[choice];
              return (
                <label key={choice} data-active={importer.media === choice}>
                  <input
                    type="radio"
                    name="luna-fetch-media"
                    value={choice}
                    checked={importer.media === choice}
                    onChange={() => importer.setMedia(choice)}
                  />
                  <Icon size={14} />
                  <span className="luna-url-import__media-text">
                    <strong>{t(`fetch.media.${choice}`)}</strong>
                    <small>{t(`fetch.media.${choice}Hint`)}</small>
                  </span>
                </label>
              );
            })}
          </div>
          {/* 缺 ffmpeg 只能取站点预合并流（通常 ≤720p）——如实说明，并给一条出路：
              安装引导集中在设置的「外部工具」面板（唯一一处，DRY），这里只负责把人送过去。 */}
          {importer.media === "video" && toolchain.status?.ffmpeg === false && (
            <p className="luna-url-import__notice">
              <span>{t("fetch.noFfmpeg")}</span>
              <button
                type="button"
                className="luna-url-import__notice-action"
                onClick={() => goTo(COMPONENTS_ROUTE)}
              >
                {t("fetch.manageTools")}
              </button>
            </p>
          )}
          {/* 「不保留媒体」的代价必须在**点开始之前**说清：源视频下架后素材就再也取不回来了。 */}
          {importer.media === "link_only" && (
            <p className="luna-url-import__notice">{t("fetch.linkOnlyNote")}</p>
          )}
        </fieldset>

        <p className="luna-url-import__consent">{t("fetch.consent")}</p>
      </div>
    );
  }

  const gate = renderToolGate();
  const canStart = importer.selected.size > 0 && !importer.submitting;

  return (
    <div
      className="luna-overlay"
      role="presentation"
      onClick={() => {
        if (!busy) {
          onClose();
        }
      }}
    >
      <div
        ref={dialogRef}
        className="luna-dialog luna-dialog--panel luna-url-import"
        role="dialog"
        aria-modal="true"
        aria-label={t("fetch.title")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-url-import__header">
          <div className="luna-url-import__title">
            <LinkIcon size={16} />
            <div className="luna-url-import__title-text">
              <strong>{t("fetch.title")}</strong>
              <small>
                {/* 如实说明"正在用哪一份"——否则会有"我明明 brew upgrade 了怎么没变"的困惑。 */}
                {toolchain.status?.version
                  ? t(
                      toolchain.status.using_managed
                        ? "fetch.toolVersionManaged"
                        : "fetch.toolVersionSystem",
                      { version: toolchain.status.version },
                    )
                  : t("fetch.subtitle")}
              </small>
            </div>
          </div>
          <button
            type="button"
            className="luna-url-import__close"
            aria-label={tCommon("action.close")}
            onClick={onClose}
          >
            <CloseIcon size={16} />
          </button>
        </header>

        <div className="luna-url-import__body">
          {gate ?? (
            <>
              {renderInstallOutcome()}
              <label className="luna-url-import__field">
                <span className="luna-url-import__field-label">{t("fetch.urlLabel")}</span>
                <input
                  ref={inputRef}
                  type="url"
                  inputMode="url"
                  autoComplete="off"
                  spellCheck={false}
                  value={draft}
                  placeholder={t("fetch.urlPlaceholder")}
                  disabled={importer.phase === "probing"}
                  onChange={(event) => {
                    setDraft(event.target.value);
                    if (importer.phase === "confirm") {
                      importer.reset(); // 改了地址 → 旧确认卡作废，避免抓到上一条
                    }
                  }}
                  onKeyDown={(event) => {
                    if (isImeKeyEvent(event)) return;
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void startProbe();
                    }
                  }}
                />
              </label>

              {importer.error && (
                <div className="luna-url-import__error" role="alert">
                  {/* 对话框里的失败必然来自抓取，故未识别码可以兜到"原因未知"。 */}
                  <p>{t(fetchErrorKey(importer.error) ?? unknownFetchErrorKey)}</p>
                  {/* 「该更新工具了」的两种说法——工具自陈过旧，以及站点在取字节这一步把我们
                      403 拒了（反爬策略变了、而我们的提取器停在安装那天）——都就地给动作，
                      而不是让用户自己走到设置页去找。仅对受管安装给（系统装的那份得由用户
                      自己 brew upgrade，我们不该去改他的 Homebrew）。 */}
                  {UPDATE_FIXES.some((code) => importer.error?.startsWith(code) === true) &&
                    toolchain.status?.using_managed === true && (
                      <button
                        type="button"
                        className="luna-btn luna-btn--primary"
                        disabled={toolchain.installing !== null || toolchain.plan === null}
                        onClick={() => void toolchain.install()}
                      >
                        <DownloadIcon size={14} />
                        <span>
                          {toolchain.installing !== null
                            ? t("fetch.installing")
                            : t("fetch.updateNow")}
                        </span>
                      </button>
                    )}
                </div>
              )}
              {importer.phase === "probing" && (
                <>
                  <p className="luna-url-import__status">
                    <span className="luna-proc-progress__spinner" aria-hidden="true" />
                    <span>{t("fetch.probing")}</span>
                    {/* 给一条**回到可编辑**的出路：粘错地址时不必等它跑完，也不必关掉重开。 */}
                    <button
                      type="button"
                      className="luna-url-import__status-cancel"
                      onClick={() => importer.reset()}
                    >
                      {tCommon("action.cancel")}
                    </button>
                  </p>
                  {/* 只在真的等久了才说话：常态下"正在读取视频信息…"加一颗取消已经够了；
                      过了十秒仍无结果，如实说是在等站点（计时，不是进度）。 */}
                  {probeSlow && <p className="luna-url-import__hint">{t("fetch.probingSlow")}</p>}
                </>
              )}
              {importer.phase === "confirm" && renderConfirm()}
            </>
          )}
        </div>

        {gate === null && (
          <footer className="luna-url-import__actions">
            {importer.phase === "confirm" ? (
              <>
                <span className="luna-url-import__count">
                  {t("fetch.selectedCount", { count: importer.selected.size })}
                </span>
                <button
                  type="button"
                  className="luna-btn luna-btn--primary"
                  disabled={!canStart}
                  onClick={() => void startFetch()}
                >
                  <DownloadIcon size={14} />
                  <span>{importer.submitting ? t("fetch.starting") : t("fetch.start")}</span>
                </button>
              </>
            ) : (
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={draft.trim().length === 0 || importer.phase === "probing"}
                onClick={() => void startProbe()}
              >
                <span>{t("fetch.probe")}</span>
              </button>
            )}
          </footer>
        )}
      </div>
    </div>
  );
}
