/**
 * 「从链接导入」应用的组件（ADR-167 ⑤，PRD-19-045 / PRD-05-007）：应用详情页「组件」一节。
 *
 * 收纳这个应用依赖的**进程外第三方可执行程序**：yt-dlp（下载网络视频，必需）、JS 运行时（yt-dlp
 * 解站点取流挑战所需，推荐）、ffmpeg（合并高清音视频流，可选）。它们原先住在设置「外部工具」页；
 * 2026-10-20 起应用可以带组件，工具的状态、安装、更新与卸载只在这里说（设置中心不再有那一页）。
 *
 * **几件工具的待遇不同，且理由要摆在界面上**：
 * - **yt-dlp** 有官方仓库 + 官方 `SHA2-256SUMS`，故 Lunarca 可以代为下载并校验（ADR-023）；
 *   且**必须给它一条更新通道**：受管安装把版本冻结在点安装的那一天，而站点的反爬策略每周
 *   都在变——2026-08-23 的真机缺陷正是一份 7 周前装好的 yt-dlp 被站点以 `HTTP 403` 拒了，
 *   而界面上没有任何地方能让用户发现"我该更新了"。「检查更新」是**用户显式触发的联网**
 *   （默认离线是铁律，AGENTS.md §7），不做后台轮询。
 * - **JS 运行时**：本机一个都没有时，Lunarca 可以代装 Deno（官方 release + 每个资产的 `.sha256sum`），
 *   代装的那份记下版本，「检查更新」一并查它（ADR-167 ⑤）；用户自己的 node / bun / deno 只检测不碰。
 * - **ffmpeg** 项目本身**不发布二进制**，官网只链第三方构建（evermeet.cx / gyan.dev / BtbN），
 *   且不为那些二进制提供官方校验和——从个人站点下载可执行文件的供应链保证远弱于前者，
 *   故只给**安装引导**，不做一键安装。这不是偷懒，是能力边界。
 *
 * **一行只留一个主动作，其余进「更多」**（2026-08-23 交互复盘）。此前 yt-dlp 那一行同屏
 * 摆着六颗同级次要按钮（安装 / 检查更新 / 重新检测 / 指定路径 / 恢复自动查找 / 卸载），四行里
 * 还各挂一颗**做同一件事**的「重新检测」。同级并列传达的是"这些同等重要、你自己挑"，而用户
 * 此刻要的恰恰是"我现在该点哪个"。现在：主动作随状态只留一颗（缺 → 安装 / 有新版 → 更新到 X /
 * 超时 → 重新检测 / 已就绪 → 检查更新），逃生口（指定路径 / 恢复自动查找 / 卸载 / 重新安装）
 * 收进溢出菜单，**「重新检测」上提为面板级的唯一一颗**——它本来就是刷新整块状态，不属于某一行。
 * 已就绪且无更新的行**一颗按钮都没有**，这才是"没事可做"的正确呈现。
 *
 * **安装前给计划，不给说明段**（PRD-05-007 2026-10-08 修订）：缺 yt-dlp 时这一行直接列出
 * 这台机器将装的几件（[`FetchInstallPlan`]，Rust 给），状态、计划、安装与结论与「从链接导入」
 * 对话框共用 [`useFetchToolchain`]——此前两处各写一份，对话框把"运行时那一半没装上"静默吞掉。
 * 手动安装的命令与"为什么要自己装"收进每行的「安装方法」折叠区。
 *
 * **安装引导由后端按平台给**（[`InstallGuide`]）：命令此前写死在 i18n 里的 `brew install …`，
 * Windows 用户复制过去必然失败，且没有第二条路。它是环境事实不是文案，见 `useInstallGuides`。
 *
 * **面板不自带标题**：详情页「组件」一节的标题由宿主给；顶部那条只有一颗「重新检测」的动作栏
 * 不是页头，只是这一整块的刷新入口。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type FetchToolUpdateDto } from "../../ipc/bindings";
import { ArrowClockwiseIcon, DownloadIcon } from "@lunarca/editor-core/Icon";
import { HudActionMenu, type HudActionMenuItem } from "../../components/ui/HudActionMenu";
import { InstallGuide } from "../../components/InstallGuide";
import { HelpDetails } from "../../components/ui/HelpDetails";
import { InlineDetails } from "../../components/ui/InlineDetails";
import { FetchInstallPlan } from "../../components/FetchInstallPlan";
import { useInstallGuides } from "../../hooks/use-install-guides";
import { useFetchToolchain } from "../../hooks/use-fetch-toolchain";
import { fetchErrorKey, unknownFetchErrorKey } from "../../lib/task-failure";
import { vaultErrorText } from "../../lib/vault-error";

/** yt-dlp 手动路径覆盖的设置键（内部键，只在这里改，ADR-167 ⑤）。 */
const TOOL_PATH_KEY = "transcribe.fetch_tool_path";

export function LinkImportComponents({ advancedMode }: { advancedMode: boolean }) {
  const { t } = useTranslation("settings");
  const { t: tTranscribe } = useTranslation("transcribe");
  const { t: tCommon } = useTranslation("common");
  const toolchain = useFetchToolchain();
  const { status, checking, installing, refresh } = toolchain;
  const [error, setError] = useState<string | null>(null);
  /** yt-dlp 的手动路径覆盖（`transcribe.fetch_tool_path`，本面板承载）。 */
  const [overridePath, setOverridePath] = useState("");
  /** 上游查更新的结果（`null` = 本次会话还没查过；不自动查，默认离线）。 */
  const [update, setUpdate] = useState<FetchToolUpdateDto | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  /** 从溢出菜单点了「重新安装」：先摊开计划再执行（重装也是一次 38 MB 起的下载）。 */
  const [confirmingReinstall, setConfirmingReinstall] = useState(false);
  /** 本机平台的安装引导（`null` = 还没取回来）。 */
  const guideFor = useInstallGuides();

  const refreshOverride = useCallback(async () => {
    const result = await commands.listSettings();
    if (result.status !== "ok") {
      return;
    }
    const value = result.data.find((s) => s.key === TOOL_PATH_KEY)?.current_value;
    setOverridePath(value?.kind === "str" ? value.value : "");
  }, []);

  /**
   * 面板级「重新检测」：一次点击刷新**整块**状态。
   *
   * 此前四行各挂一颗，其中三颗调的是同一个 `fetchToolStatus`——同一个动作重复四遍，
   * 却让人以为它们各管各的。工具状态本就是一次探测的整体结果，刷新入口就该只有一处。
   */
  const recheckAll = useCallback(async () => {
    await refresh(true);
  }, [refresh]);

  useEffect(() => {
    void refreshOverride();
  }, [refreshOverride]);

  async function pickToolPath() {
    const picked = await commands.pickFile();
    if (picked.status !== "ok" || picked.data === null) {
      return;
    }
    await commands.setSetting(TOOL_PATH_KEY, { kind: "str", value: picked.data });
    await refreshOverride();
    await refresh(true);
  }

  async function clearToolPath() {
    await commands.setSetting(TOOL_PATH_KEY, { kind: "str", value: "" });
    await refreshOverride();
    await refresh(true);
  }

  /**
   * 按计划安装（yt-dlp，以及**本机一个 JS 运行时都没有时**顺带装上的 Deno）。
   *
   * 运行时那一半失败**不算整次失败**——yt-dlp 已经能用了，说它失败是撒谎；但也不能沉默，
   * 结论由 `toolchain.outcome.js_runtime_error` 在运行时那一行说出口。
   */
  async function install() {
    setError(null);
    setConfirmingReinstall(false);
    if (await toolchain.install()) {
      // 装的就是 latest：旧的查询结论（"有新版 X"）此刻已作废，留着它会继续劝用户更新。
      setUpdate(null);
    }
  }

  /** 卸载受管 JS 运行时（约 77 MB；装得上就必须删得掉）。 */
  async function uninstallJsRuntime() {
    setError(null);
    const result = await commands.uninstallFetchJsRuntime();
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      return;
    }
    await refresh(true);
  }

  /**
   * 查上游有没有新版（**用户点了才联网**）。
   *
   * 这是「受管安装」缺失的另一半：装完那一刻它是 latest，此后就永远停在那一天，
   * 而站点的反爬策略每周都在变——下载忽然全部失败时，用户没有任何线索指向"该更新工具了"。
   */
  async function checkUpdate() {
    setError(null);
    setCheckingUpdate(true);
    try {
      const result = await commands.checkFetchToolUpdate();
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        return;
      }
      setUpdate(result.data);
    } finally {
      setCheckingUpdate(false);
    }
  }

  async function uninstall() {
    setError(null);
    const result = await commands.uninstallFetchTool();
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      return;
    }
    setUpdate(null);
    await refresh(true);
  }

  const busy = checking || installing !== null || checkingUpdate;
  const jsRuntimeError = toolchain.runtimeError;
  const managed = status?.managed_version !== null && status?.managed_version !== undefined;
  // 未取回检测结果不等于缺失；恢复警告只跟随 Rust 已返回的检测事实。
  const unknownState = checking ? "checking" : "unavailable";
  const jsRuntimeState = status === null ? unknownState : status.js_runtime ? "ready" : "recommended";
  /** 代装的 Deno 查出有新版（「检查更新」一并查它，ADR-167 ⑤）。 */
  const jsUpdate = update?.js_runtime ?? null;
  const ffmpegState = status === null ? unknownState : status.ffmpeg ? "ready" : "missing";
  // yt-dlp 的四种处境（与「从链接导入」对话框同一口径，DRY 的是判据不是代码）。
  const ytState =
    status === null
      ? unknownState
      : status.version !== null
        ? "ready"
        : status.tool_path === null
          ? "missing"
          : status.probe_timed_out
            ? "timeout"
            : "broken";

  /**
   * yt-dlp 这一行的**唯一主动作**：状态决定它是哪一个，而不是把五个动作并排让用户猜。
   *
   * - `timeout` → 重新检测（工具多半是好的，只是启动慢，重装是错的建议）
   * - 查出有新版 → 更新到 X（这正是"下载忽然全失败"时要点的那一下）
   * - 已就绪且受管 → 检查更新（唯一还值得做的事）
   * - 已就绪但用的是系统安装 → 没有主动作（更新归 brew / winget 管，我们不越俎代庖）
   * - 其余（没装 / 跑不起来）→ 让 Lunarca 安装
   */
  const ytPrimary: "recheck" | "update" | "checkUpdate" | "install" | null =
    status === null
      ? null
      : ytState === "timeout"
      ? "recheck"
      : update?.update_available === true
        ? "update"
        : ytState === "ready"
          ? managed
            ? "checkUpdate"
            : null
          : "install";

  const ytMenuItems: HudActionMenuItem[] = [
    ...(managed && ytPrimary !== "checkUpdate" && ytPrimary !== "update"
      ? [
          {
            id: "check-update",
            label: t("tools.check_update"),
            disabled: busy,
            onSelect: () => void checkUpdate(),
          },
        ]
      : []),
    ...(ytPrimary === "install"
      ? []
      : [
          {
            id: "reinstall",
            label: managed ? tTranscribe("fetch.reinstall") : tTranscribe("fetch.installNow"),
            disabled: busy,
            onSelect: () => setConfirmingReinstall(true),
          },
        ]),
    // 手动指定路径：这一项此前是独立设置行，与本面板同屏显示"未设置"而面板显示"可用"，
    // 两个说法互相矛盾（真机反馈）。收进工具自己这一行后，一件工具的状态与配置只有一处说法。
    // 它是**逃生口**（装在非常规位置才需要），故归菜单而不是与主动作并列。
    { id: "set-path", label: t("tools.set_path"), disabled: busy, onSelect: () => void pickToolPath() },
    ...(overridePath !== ""
      ? [
          {
            id: "auto-detect",
            label: t("tools.auto_detect"),
            disabled: busy,
            onSelect: () => void clearToolPath(),
          },
        ]
      : []),
    // 卸载只对**我们自己装的那份**出现——绝不去删用户 brew / winget 装的东西。
    ...(managed
      ? [
          {
            id: "uninstall",
            label: t("tools.uninstall"),
            tone: "danger" as const,
            disabled: busy,
            onSelect: () => void uninstall(),
          },
        ]
      : []),
  ];

  return (
    // 应用详情页「组件」一节：标题由宿主给，面板**不写**第二遍。
    <section className="luna-settings-panel" aria-label={t("tools.title")}>
      {/* 管辖边界不是"这一节叫什么"的复述，而是一条用户问得出、别处没写的事实：
          列表里显示「可用」的那个 ffmpeg 可能是 Homebrew 装的，Lunarca 不会去动它。 */}
      {advancedMode ? <p className="luna-settings-panel__hint">{t("tools.managed_scope")}</p> : null}
      {/* 面板级动作栏：全应用唯一的一颗「重新检测」（不是页头——无标题、无说明）。 */}
      <div className="luna-settings-tools__bar">
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          disabled={busy}
          onClick={() => void recheckAll()}
        >
          <ArrowClockwiseIcon size={14} />
          <span>{checking ? tTranscribe("fetch.checkingTool") : t("tools.recheck_all")}</span>
        </button>
      </div>

      <ul className="luna-settings-tools__list">
        {/* ── yt-dlp：Lunarca 可代装（官方仓库 + 官方校验和）────────────────────── */}
        <li
          className="luna-settings-tools__item"
          data-state={ytState}
          data-setting-key={TOOL_PATH_KEY}
          tabIndex={-1}
        >
          <div className="luna-settings-tools__item-main">
            <div className="luna-settings-tools__item-head">
              <strong>{t("tools.yt_dlp_label")}
                {advancedMode && managed ? (
                  <HelpDetails summary={t("tools.yt_dlp_label")}>
                    <p>{t("tools.update_hint")}</p>
                  </HelpDetails>
                ) : null}
              </strong>
              <span className="luna-settings-tools__badge" data-state={ytState}>
                {t(`tools.state.${ytState}`)}
              </span>
              {status?.using_managed === true && (
                <span className="luna-settings-tools__badge">{t("tools.managed")}</span>
              )}
            </div>
            <p className="luna-settings-tools__item-desc">{t("tools.yt_dlp_desc")}</p>
            {ytState === "missing" ? <InlineDetails summary={t("tools.manual_install")}>
              <InstallGuide guide={guideFor?.("yt_dlp") ?? null} />
            </InlineDetails> : null}
            {advancedMode && status?.version !== null && status?.version !== undefined && (
              <p className="luna-settings-tools__item-meta">
                {t("tools.version", { version: status.version })}
              </p>
            )}
            {advancedMode && status?.tool_path !== null && status?.tool_path !== undefined && (
              <p className="luna-settings-tools__item-meta" title={status.tool_path}>
                {status.tool_path}
              </p>
            )}
            {advancedMode && overridePath !== "" && (
              <p className="luna-settings-tools__item-meta" title={overridePath}>
                {t("tools.override_path", { path: overridePath })}
              </p>
            )}
            {installing !== null && (
              <p className="luna-settings-tools__item-meta">
                {installing.progress === ""
                  ? tTranscribe("fetch.installStarting")
                  : tTranscribe("fetch.installProgress", {
                      name: installing.name,
                      progress: installing.progress,
                    })}
              </p>
            )}
            {/* 查更新的结论如实说：有新版就报版本号（主按钮同时改口叫「更新到 X」），
                没有就明说"已是最新"——不给结论的"检查完了"等于没查。 */}
            {update !== null && (
              <p className="luna-settings-tools__item-meta">
                {update.update_available
                  ? t("tools.update_available", { version: update.latest_version })
                  : t("tools.up_to_date", { version: update.latest_version })}
              </p>
            )}
            {/* 受管那份才需要这句：系统装的由它自己的包管理器管更新，我们不该越俎代庖。 */}
            {/* 要下载的时候（安装 / 更新 / 从菜单重装）先摊开计划：装哪几件、从哪来、约多大、装到哪。 */}
            {(ytPrimary === "install" || ytPrimary === "update" || confirmingReinstall) && (
              <FetchInstallPlan plan={toolchain.plan} />
            )}
            {toolchain.installError !== null && (
              <p className="luna-settings-tools__error" role="alert">
                {tTranscribe(fetchErrorKey(toolchain.installError) ?? unknownFetchErrorKey)}
              </p>
            )}
            {toolchain.outcome !== null && toolchain.outcome.js_runtime_error === null && (
              <p className="luna-settings-tools__item-meta" role="status">
                {toolchain.outcome.js_runtime_version === null
                  ? tTranscribe("fetch.installDone", { version: toolchain.outcome.tool_version })
                  : tTranscribe("fetch.installDoneWithRuntime", {
                      version: toolchain.outcome.tool_version,
                      runtime: toolchain.outcome.js_runtime_version,
                    })}
              </p>
            )}
          </div>
          <div className="luna-settings-tools__item-actions">
            {ytPrimary === "recheck" && (
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={busy}
                onClick={() => void refresh(true)}
              >
                <ArrowClockwiseIcon size={14} />
                <span>{checking ? tTranscribe("fetch.checkingTool") : tTranscribe("fetch.recheck")}</span>
              </button>
            )}
            {ytPrimary === "update" && (
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={busy || toolchain.plan === null}
                onClick={() => void install()}
              >
                <DownloadIcon size={14} />
                <span>{t("tools.update_to", { version: update?.latest_version ?? "" })}</span>
              </button>
            )}
            {advancedMode && ytPrimary === "checkUpdate" && (
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                disabled={busy}
                onClick={() => void checkUpdate()}
              >
                {checkingUpdate ? t("tools.checking_update") : t("tools.check_update")}
              </button>
            )}
            {(ytPrimary === "install" || confirmingReinstall) && (
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={busy || toolchain.plan === null}
                onClick={() => void install()}
              >
                <DownloadIcon size={14} />
                <span>
                  {managed ? tTranscribe("fetch.reinstall") : tTranscribe("fetch.installNow")}
                </span>
              </button>
            )}
            {confirmingReinstall && (
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                disabled={busy}
                onClick={() => setConfirmingReinstall(false)}
              >
                {tCommon("action.cancel")}
              </button>
            )}
            {advancedMode ? <HudActionMenu
              label={t("tools.more_actions", { tool: t("tools.yt_dlp_label") })}
              items={ytMenuItems}
              align="end"
              side="bottom"
            /> : null}
          </div>
        </li>

        {/* ── ffmpeg：只引导不代装（上游无官方二进制、无官方校验和）──────────
            装好了就**一颗按钮都没有**——"没事可做"该长成没有按钮的样子。 */}
        <li className="luna-settings-tools__item" data-state={ffmpegState}>
          <div className="luna-settings-tools__item-main">
            <div className="luna-settings-tools__item-head">
              <strong>{t("tools.ffmpeg_label")}</strong>
              <span
                className="luna-settings-tools__badge"
                data-state={ffmpegState}
              >
                {t(ffmpegState === "missing" ? "tools.state.optional" : `tools.state.${ffmpegState}`)}
              </span>
            </div>
            <p className="luna-settings-tools__item-desc">{t("tools.ffmpeg_desc")}</p>
            {ffmpegState === "missing" ? <>
              <p className="luna-settings-tools__item-meta">{t("tools.ffmpeg_missing")}</p>
              <InlineDetails summary={t("tools.install_method")}>
                <p>{t("tools.ffmpeg_why_manual")}</p>
                <InstallGuide guide={guideFor?.("ffmpeg") ?? null} />
              </InlineDetails>
            </> : null}
            {status?.ffmpeg_path !== null && status?.ffmpeg_path !== undefined ? (
              advancedMode ? <p className="luna-settings-tools__item-meta" title={status.ffmpeg_path}>
                {status.ffmpeg_path}
              </p> : null
            ) : null}
          </div>
        </li>

        {/* ── JS 运行时：只检测不代装（与 ffmpeg 同一条能力边界）──────────────
            徽标用 `recommended` 而不是 ffmpeg 的 `optional`：缺 ffmpeg 只是画质降档
            （仍下得下来），缺 JS 运行时却会让 yt-dlp 退到备用播放器客户端——那些地址随时
            可能被站点 403 拒掉，即"可能根本下不下来"。两种缺失的分量不同，措辞就不该相同。 */}
        <li className="luna-settings-tools__item" data-state={jsRuntimeState}>
          <div className="luna-settings-tools__item-main">
            <div className="luna-settings-tools__item-head">
              <strong>{t("tools.js_runtime_label")}</strong>
              <span className="luna-settings-tools__badge" data-state={jsRuntimeState}>
                {t(`tools.state.${jsRuntimeState}`)}
              </span>
              {status?.using_managed_js === true && (
                <span className="luna-settings-tools__badge">{t("tools.managed")}</span>
              )}
            </div>
            <p className="luna-settings-tools__item-desc">{t("tools.js_runtime_desc")}</p>
            {jsRuntimeState === "recommended" ? <>
              <p className="luna-settings-tools__item-meta">{t("tools.js_runtime_missing")}</p>
              <InlineDetails summary={t("tools.install_method")}>
                <p>{t("tools.js_runtime_why_manual")}</p>
                <InstallGuide guide={guideFor?.("js_runtime") ?? null} />
              </InlineDetails>
            </> : null}
            {advancedMode && status?.using_managed_js === true && status.managed_js_version !== null ? (
              <p className="luna-settings-tools__item-meta">
                {t("tools.version", { version: status.managed_js_version })}
              </p>
            ) : null}
            {jsUpdate !== null ? (
              <p className="luna-settings-tools__item-meta">
                {jsUpdate.update_available
                  ? t("tools.update_available", { version: jsUpdate.latest_version })
                  : t("tools.up_to_date", { version: jsUpdate.latest_version })}
              </p>
            ) : null}
            {status?.js_runtime_path !== null && status?.js_runtime_path !== undefined ? (
              advancedMode ? <p className="luna-settings-tools__item-meta" title={status.js_runtime_path}>
                {status.js_runtime_path}
              </p> : null
            ) : (
              <>
                {/* 计划里确实有它时才说"跟着 yt-dlp 一起装"——计划由 Rust 按本机情况给，
                    不再是一句恒定的承诺。 */}
                {ytState !== "ready" &&
                  toolchain.plan?.components.some((part) => part.tool === "js_runtime") === true && (
                    <p className="luna-settings-tools__item-meta">{t("tools.js_runtime_in_plan")}</p>
                  )}
              </>
            )}
            {/* 合并安装里运行时那一半没成：yt-dlp 已经能用，故这不是整块失败，但必须说出口。 */}
            {jsRuntimeError !== null && (
              <p className="luna-settings-tools__error" role="alert">
                {t("tools.js_runtime_install_failed", {
                  reason: tTranscribe(fetchErrorKey(jsRuntimeError) ?? unknownFetchErrorKey),
                })}
              </p>
            )}
          </div>
          <div className="luna-settings-tools__item-actions">
            {/* 缺运行时且 yt-dlp 已就绪 → 这一行自己就能装。首次安装不会走到这里
                （那时 yt-dlp 也没装，一颗按钮一次装齐）。 */}
            {jsRuntimeState === "recommended" && ytState === "ready" && (
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={busy}
                onClick={() => void toolchain.installJsRuntime()}
              >
                <DownloadIcon size={14} />
                <span>{tTranscribe("fetch.installNow")}</span>
              </button>
            )}
            {/* 代装的 Deno 有新版：更新 = 重新取 latest 并校验（与首次安装同一条路）。 */}
            {jsUpdate?.update_available === true && status?.using_managed_js === true && (
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                disabled={busy}
                onClick={() =>
                  void toolchain.installJsRuntime().then(() => {
                    setUpdate((current) => (current ? { ...current, js_runtime: null } : current));
                  })
                }
              >
                <DownloadIcon size={14} />
                <span>{t("tools.update_to", { version: jsUpdate.latest_version })}</span>
              </button>
            )}
            {/* 卸载只对受管那份出现（约 77 MB，装得上就必须删得掉）；用户自己装的绝不去动。 */}
            {advancedMode && status?.using_managed_js === true && (
              <HudActionMenu
                label={t("tools.more_actions", { tool: t("tools.js_runtime_label") })}
                items={[
                  {
                    id: "uninstall-js",
                    label: t("tools.uninstall"),
                    tone: "danger" as const,
                    disabled: busy,
                    onSelect: () => void uninstallJsRuntime(),
                  },
                ]}
                align="end"
                side="bottom"
              />
            )}
          </div>
        </li>

      </ul>

      {/* 模型库（拉取/删除 LLM 权重）仍在「AI 引擎 / 模型」——那里回答的是"用哪个模型"，
          与"运行时是否安装"分属两件事，合并会让两边都说不清。 */}
      <p className="luna-settings-tools__footnote">{t("tools.models_elsewhere")}</p>

      {error !== null && (
        <p className="luna-settings-tools__error" role="alert">
          {tTranscribe(fetchErrorKey(error) ?? unknownFetchErrorKey)}
        </p>
      )}
    </section>
  );
}
