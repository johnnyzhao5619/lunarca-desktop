/**
 * 一个一方应用的动作（ADR-168 ① ③ ④）：主按钮、暂时停用、卸载，列表行与详情页共用这一份。
 *
 * 主按钮是哪一个由 Rust 定（`AppDto.action`），这里只把它接到对应的命令上：
 * - 安装与修复走同一份计划（`app_install_plan`），先弹一张 Chrome 式的确认框——「它可以：」逐条
 *   权限，要下载的东西合成一行写总大小（`InstallConfirmContent`）；确认后 `app_install`。下载期间
 *   主按钮上显示进度（认领 `model://download-progress` 里计划给出的 `progress_id`）。
 * - 暂时停用与卸载都先确认、确认框写明库里的内容不动。
 *
 * 写回之后由 `settings://changed` 驱动重读（`useApps`），不乐观置位。刚装上（或刚启用）时详情页
 * 顶上出现这个应用的入门；从列表行装的由调用方带去详情页，入门在那里出现（`onEnabled`）。
 */

import { createElement, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, events, type AppDto } from "../../ipc/bindings";
import { useConfirm } from "../../components/ConfirmHost";
import { installApp, refreshApps, setAppEnabled, uninstallApp } from "../../hooks/use-apps";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { goTo } from "../../lib/navigate";
import { InstallConfirmContent } from "./InstallConfirmContent";

/** 严格本地开关的深链（要下载的安装、修复与更新都要先关掉它）。 */
const STRICT_LOCAL_ROUTE = "settings://privacy/privacy.strict_local";

/**
 * 装上之后、换到详情页之前「要出入门」的应用 id（从列表行装上时由那一行放进来，详情页挂上时取走）。
 * 不落盘：离开详情页或点「跳过」就收起（ADR-165 R6）。
 */
const pendingSetup = new Set<string>();

/** 上一次操作的结果提示（文案键）。 */
export type AppActionNotice = "apps.saveFailed" | "apps.install.failed" | "apps.install.limited" | "apps.update.failed";

export interface AppAction {
  id: string;
  label: string;
  run: () => void;
}

export interface AppActions {
  /** 主按钮；这一态没有可做的就是 `null`（如没有空间、也还没有「去使用」入口的应用）。 */
  primary: AppAction | null;
  /** 暂时停用（只在生效时有）。 */
  pause: AppAction | null;
  /** 卸载（只在已安装时有）。 */
  uninstall: AppAction | null;
  busy: boolean;
  /** 安装或修复进行中按钮上那句（「正在下载 45%」）；没在装为 `null`。 */
  progressLabel: string | null;
  notice: AppActionNotice | null;
  /** 提示是不是失败（受限也是提示，但不是失败）。 */
  failed: boolean;
  /** 这一次刚装上或刚启用（入门卡据此出现；离开或点「跳过」就收起，不落盘）。 */
  justEnabled: boolean;
  dismissSetup: () => void;
}

export interface AppActionOptions {
  /** 装上或启用成功之后（列表行用它把人带去详情页看入门）。 */
  onEnabled?: () => void;
}

/** 一次安装或更新的下载进度：每件组件下了多少、一共多少（事件没给总数时用计划里的约数）。 */
type DownloadProgress = Record<string, { downloaded: number; total: number; done: boolean }>;

/** 这一次要下载的一件：进度事件里的 id，以及事件给出总数之前用的约数。 */
interface PendingDownload {
  progress_id: string;
  approx_download_bytes: number | null;
}

function progressPercent(downloads: readonly PendingDownload[], progress: DownloadProgress): number {
  let done = 0;
  let total = 0;
  for (const item of downloads) {
    const entry = progress[item.progress_id];
    const size = entry && entry.total > 0 ? entry.total : (item.approx_download_bytes ?? 0);
    total += size;
    done += entry ? (entry.done ? size : Math.min(entry.downloaded, size)) : 0;
  }
  return total > 0 ? Math.floor((done / total) * 100) : 0;
}

export function useAppActions(app: AppDto, options: AppActionOptions = {}): AppActions {
  const { t, i18n } = useTranslation("workspace");
  const confirm = useConfirm();
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<AppActionNotice | null>(null);
  const [justEnabled, setJustEnabled] = useState(() => pendingSetup.has(app.id));
  /** 正在装的这一份计划里要下载的东西（进度按它们认领事件）；没在装为 `null`。 */
  const [installing, setInstalling] = useState<readonly PendingDownload[] | null>(null);
  const [progress, setProgress] = useState<DownloadProgress>({});
  const name = t(`apps.${app.id}.name`);
  const latestOptions = useRef(options);
  latestOptions.current = options;

  // 详情页挂上时取走「要出入门」的标记：它只管这一次换页。
  useEffect(() => {
    pendingSetup.delete(app.id);
  }, [app.id]);

  useEffect(() => {
    if (installing === null || installing.length === 0) return;
    const ids = new Set(installing.map((item) => item.progress_id));
    const unlisten = events.modelDownloadProgress.listen((event) => {
      const payload = event.payload;
      if (!ids.has(payload.model_id)) return;
      setProgress((current) => ({
        ...current,
        [payload.model_id]: {
          downloaded: payload.downloaded_bytes ?? 0,
          total: payload.total_bytes ?? 0,
          done: payload.done,
        },
      }));
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [installing]);

  /** 跑一个动作：`"cancelled"` = 用户在确认框里取消了，什么都没发生，也不说话。 */
  const run = async (work: () => Promise<boolean | "cancelled">, enabling: boolean) => {
    setBusy(true);
    setNotice(null);
    const saved = await work();
    setBusy(false);
    setInstalling(null);
    setProgress({});
    if (saved === "cancelled") return;
    if (!saved) {
      setNotice((current) => current ?? "apps.saveFailed");
      return;
    }
    if (!enabling) return;
    const onEnabled = latestOptions.current.onEnabled;
    if (onEnabled) {
      pendingSetup.add(app.id);
      onEnabled();
    } else {
      setJustEnabled(true);
    }
  };

  /** 安装与修复：取计划 → 确认（它可以什么、要下载多少）→ 交回计划安装（ADR-168 ③）。 */
  const install = (repair: boolean) =>
    void run(async (): Promise<boolean | "cancelled"> => {
      const plan = await commands.appInstallPlan(app.id);
      if (plan.status !== "ok") return false;
      const downloads = plan.data.downloads;
      const bytes = downloads.reduce((sum, item) => sum + (item.approx_disk_bytes ?? 0), 0);
      const ok = await confirm({
        title: t(repair ? "apps.repair.title" : "apps.install.title", { name }),
        content: createElement(InstallConfirmContent, {
          permissions: repair ? [] : plan.data.permissions,
          downloadSize: downloads.length > 0 ? formatBytes(bytes, i18n.language) : null,
          usesExisting: plan.data.present.length > 0,
        }),
        confirmLabel: t(repair ? "apps.action.repair" : "apps.action.install"),
        destructive: false,
      });
      if (!ok) return "cancelled";
      setInstalling(downloads);
      const result = await installApp(
        app.id,
        downloads.map((item) => item.id),
      );
      if (!result.ok) {
        setNotice("apps.install.failed");
        return false;
      }
      if (result.limitedBy.length > 0) setNotice("apps.install.limited");
      return true;
    }, true);

  /** 更新：只重装检查过、确有新版的那几件 Lunarca 下载的组件。 */
  const update = () =>
    void run(async () => {
      setInstalling(
        app.components.flatMap((component) =>
          component.update_available && component.progress_id !== null
            ? [{ progress_id: component.progress_id, approx_download_bytes: null }]
            : [],
        ),
      );
      const result = await commands.appUpdate(app.id);
      // 更新不改设置，没有 `settings://changed` 来推着重读：自己重取一次。
      refreshApps();
      if (result.status !== "ok") {
        setNotice("apps.update.failed");
        return false;
      }
      return true;
    }, false);

  const primary = ((): AppAction | null => {
    switch (app.action) {
      case "install":
        return { id: "install", label: t("apps.action.install"), run: () => install(false) };
      case "repair":
        return { id: "repair", label: t("apps.action.repair"), run: () => install(true) };
      case "enable":
        return {
          id: "enable",
          label: t("apps.action.enable"),
          run: () => void run(() => setAppEnabled(app.id, true), true),
        };
      case "leave_strict_local":
        return { id: "leave_strict_local", label: t("apps.action.leave_strict_local"), run: () => goTo(STRICT_LOCAL_ROUTE) };
      case "open":
        return app.space ? { id: "open", label: t("apps.action.open"), run: () => goTo(`${app.space}://`) } : null;
      case "update":
        // 更新是用户点了才发生的一次下载（ADR-168 ⑧）：不另弹确认，进度照样上按钮。
        return { id: "update", label: t("apps.action.update"), run: update };
    }
  })();

  const pause: AppAction | null = app.enabled
    ? {
        id: "pause",
        label: t("apps.disable.action"),
        run: () =>
          void (async () => {
            const ok = await confirm({
              title: t("apps.disable.title", { name }),
              description: t("apps.disable.description", { name }),
              confirmLabel: t("apps.disable.confirm"),
              destructive: false,
            });
            if (ok) await run(() => setAppEnabled(app.id, false), false);
          })(),
      }
    : null;

  const uninstall: AppAction | null = app.installed
    ? {
        id: "uninstall",
        label: t("apps.uninstall.action"),
        run: () =>
          void (async () => {
            const ok = await confirm({
              title: t("apps.uninstall.title", { name }),
              description:
                app.size_bytes === null
                  ? t("apps.uninstall.description")
                  : t("apps.uninstall.descriptionWithComponents", {
                      size: formatBytes(app.size_bytes, i18n.language),
                    }),
              confirmLabel: t("apps.uninstall.confirm"),
              destructive: true,
            });
            if (ok) await run(() => uninstallApp(app.id), false);
          })(),
      }
    : null;

  const progressLabel = !busy || installing === null
    ? null
    : installing.length === 0
      ? t("apps.progress.installing")
      : (() => {
          const percent = progressPercent(installing, progress);
          return percent >= 100 ? t("apps.progress.finishing") : t("apps.progress.downloading", { percent });
        })();

  return {
    primary,
    pause,
    uninstall,
    busy,
    progressLabel,
    notice,
    failed: notice !== null && notice !== "apps.install.limited",
    justEnabled,
    dismissSetup: () => setJustEnabled(false),
  };
}
