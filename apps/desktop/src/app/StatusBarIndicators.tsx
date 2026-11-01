/**
 * 工作区底栏上那几颗**自带订阅**的指示（PRD-07-045 / PRD-04-033）。
 *
 * **高频订阅住在渲染它的叶子里，不在工作区根**（2026-10-15 真机卡顿）：资源占用每约 1.7 秒
 * 推一次、同步每一轮发好几次总览、录音计时每秒走一格、索引扫描逐批报进度——此前这些都是
 * `WorkspaceApp` 的 state，于是每一拍都把整个工作区（编辑器、回放、知识库页）重渲染一遍。
 * 搬到这里之后，一拍只重渲染底栏上那一颗。
 *
 * 前端零业务（ADR-003）：判定与投影仍是 Rust 快照 + 各自的纯函数，这里只取数、订阅、渲染。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events } from "../ipc/bindings";
import { BellIcon, CpuIcon } from "../components/ui/Icon";
import { useComputeStatus } from "../hooks/use-compute-status";
import { useIndexStatus } from "../hooks/use-index-status";
import { useSyncOverviewSelector } from "../hooks/use-sync-overview";
import { useNotificationUnread } from "../features/notifications/use-notification-unread";
import { projectStatusBarCompute } from "../features/workspace/statusbar-compute-status";
import {
  projectStatusBarSync,
  type StatusBarSyncKind,
  type StatusBarSyncProjection,
} from "../features/workspace/statusbar-sync-status";
import {
  recordingIndicatorElapsedMs,
  type RecordingIndicator,
} from "../features/workspace/recording-indicator";
import { formatClock } from "../lib/format-duration";
import { goTo } from "../lib/navigate";

/** 底栏同步指示里要用户动手的那几种（加警示色）；其余是安静的状态。 */
const SYNC_ATTENTION: ReadonlySet<StatusBarSyncKind> = new Set([
  "awaiting_confirmation",
  "locked",
  "unavailable",
  "read_only",
  "error",
]);

/**
 * 「录音中 · 计时」（B2，PRD-07-045 / PRD-04-003）。
 *
 * 录音状态的**转移**由宿主订阅（活动栏也要据此让「录音」那格在场），每秒走的那只钟只属于
 * 这颗按钮：录音中每秒推进；暂停 / 停止中冻结（纯渲染时钟，判定在 recording-indicator.ts）。
 * 点它回到录音面板，而不是录音列表——面板才是现场。
 */
export function StatusBarRecording({ indicator }: { indicator: RecordingIndicator | null }) {
  const { t } = useTranslation("workspace");
  const [clock, setClock] = useState(() => Date.now());
  useEffect(() => {
    if (indicator?.status !== "recording") return;
    setClock(Date.now());
    const timer = window.setInterval(() => setClock(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [indicator]);
  if (!indicator) {
    return null;
  }
  const label = t(
    indicator.status === "paused"
      ? "statusbar.recordingPaused"
      : indicator.status === "stopping"
        ? "statusbar.recordingStopping"
        : "statusbar.recordingElapsed",
    { time: formatClock(recordingIndicatorElapsedMs(indicator, clock)) },
  );
  return (
    <button
      type="button"
      className="luna-workspace__statusbar-recording"
      data-recording="true"
      aria-label={label}
      onClick={() => void commands.recordingShowHud()}
    >
      <span aria-hidden="true" className="luna-pulse-dot">
        ⏺
      </span>
      <span>{label}</span>
    </button>
  );
}

/**
 * 状态栏左侧那句话：索引扫描中报进度（PRD-06-004 ④），否则是宿主的状态文案。
 *
 * 单行、放不下就省略（整句在 title 里）：状态栏行高固定，长报错折行会溢进主区、把右侧各项
 * 挤成竖排（E2 抽样 2026-10-06，960 宽 150% 字号）。
 */
export function StatusBarMessage({ status }: { status: string }) {
  const { t } = useTranslation("workspace");
  // 增量扫描只数变了的篇；什么都没变时为 null，状态栏不闪一下。
  const indexStatus = useIndexStatus();
  const message = indexStatus
    ? t("status.indexing", { indexed: indexStatus.indexed, total: indexStatus.total })
    : status;
  return (
    <span className="luna-workspace__statusbar-message" title={message || undefined}>
      {message}
    </span>
  );
}

/** 底栏显示的取整占用；`null` = 尚未采样（f32 经 specta 为 number|null）。 */
interface ResourceUsageView {
  cpu: number;
  gpu: number | null;
}

/**
 * CPU / GPU 收进一颗按钮（R5，2026-10-08）：它不是任务、错误、保存或执行位置那类需要常显的
 * 状态。数值始终在按钮的可访问名里（读屏与悬停可得），点一下就地展开；收起只改呈现，采样照常
 * （Rust 侧不因此停采或改频）。
 *
 * **取整只在这里做一次**，并按取整后的值比较：浮点占用几乎每拍都变，取整后多半没变，没变就
 * 连这颗按钮也不重渲染。
 */
export function StatusBarResource() {
  const { t } = useTranslation("workspace");
  const [usage, setUsage] = useState<ResourceUsageView | null>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const unlisten = events.resourceUsage.listen((event) => {
      const cpu = Math.round(event.payload.cpu_percent ?? 0);
      const gpu = event.payload.gpu_percent === null ? null : Math.round(event.payload.gpu_percent);
      setUsage((previous) =>
        previous !== null && previous.cpu === cpu && previous.gpu === gpu ? previous : { cpu, gpu },
      );
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);
  if (!usage) {
    return null;
  }
  /** CPU / GPU 的可访问名：按钮收起时数值也在这里（读屏与悬停）。 */
  const label =
    usage.gpu !== null
      ? t("statusbar.resourceLabelGpu", { cpu: usage.cpu, gpu: usage.gpu })
      : t("statusbar.resourceLabel", { cpu: usage.cpu });
  return (
    <button
      type="button"
      className="luna-workspace__statusbar-item luna-workspace__statusbar-res"
      aria-pressed={open}
      aria-label={label}
      title={label}
      onClick={() => setOpen((current) => !current)}
    >
      <CpuIcon size={13} />
      {open && (
        <>
          <span className="luna-workspace__statusbar-res-metric">
            {t("statusbar.cpu", { pct: usage.cpu })}
          </span>
          {usage.gpu !== null && (
            <span className="luna-workspace__statusbar-res-metric">
              {t("statusbar.gpu", { pct: usage.gpu })}
            </span>
          )}
        </>
      )}
    </button>
  );
}

/**
 * 状态栏的算力标签是一个入口：点它直达「AI 算力与模型」——算力卡片就在那一页首屏，缺账户时
 * 卡片自带「登录」动作（PRD-09-080），此处不再另造一条去向。与算力卡、账户页同一份快照：
 * 登录 / 退出 / 隐私档一变，三处同一帧换说法。
 */
export function StatusBarCompute() {
  const { t } = useTranslation("workspace");
  const projection = projectStatusBarCompute(useComputeStatus());
  const label = (() => {
    switch (projection.kind) {
      case "managed_service":
        return t("statusbar.compute.managedService");
      case "device_ready":
        return t("statusbar.compute.deviceReady", {
          model: projection.modelName ?? t("statusbar.compute.deviceModelMissing"),
        });
      case "device_model_missing":
        return t("statusbar.compute.deviceModelMissing");
      case "device_runtime_unavailable":
        return t("statusbar.compute.deviceRuntimeUnavailable");
      case "loading":
        return t("statusbar.compute.loading");
      case "unavailable":
        return t("statusbar.compute.unavailable");
    }
  })();
  return (
    <button
      type="button"
      className="luna-workspace__statusbar-compute-btn"
      title={t("statusbar.computeTitle")}
      onClick={() => {
        goTo("settings://ai_services");
      }}
    >
      {label}
    </button>
  );
}

function sameSyncProjection(previous: StatusBarSyncProjection, next: StatusBarSyncProjection) {
  return previous.kind === next.kind && previous.count === next.count;
}

/**
 * 同步指示只在开了同步时出现；点它去账户页「云同步」一节——确认删除、解锁、停用都在那里，
 * 底栏不另造第二个入口。
 *
 * 只订聚合出来的那一个状态：一轮同步里每库的阶段、字节数、完成时刻会变好几次，而底栏只在
 * 「该说哪一句」变了时才需要重渲染。
 */
export function StatusBarSync() {
  const { t } = useTranslation("workspace");
  const sync = useSyncOverviewSelector(projectStatusBarSync, sameSyncProjection);
  if (sync.kind === "hidden") {
    return null;
  }
  const count = sync.count;
  const label = (() => {
    switch (sync.kind) {
      case "awaiting_confirmation":
        return count > 0
          ? t("statusbar.sync.awaitingConfirmation", { count })
          : t("statusbar.sync.awaitingConfirmationEmpty");
      case "locked":
        return t("statusbar.sync.locked");
      case "unavailable":
        return t("statusbar.sync.unavailable");
      case "read_only":
        return t("statusbar.sync.readOnly");
      case "account_frozen":
        return t("statusbar.sync.accountFrozen");
      case "offline":
        return t("statusbar.sync.offline");
      case "error":
        return t("statusbar.sync.error");
      case "paused":
        return t("statusbar.sync.paused");
      case "large_paused_metered":
        return t("statusbar.sync.largePausedMetered");
      case "large_paused_battery":
        return t("statusbar.sync.largePausedBattery");
      case "syncing":
        return t("statusbar.sync.syncing");
      case "conflicts":
        return t("statusbar.sync.conflicts", { count });
      case "synced":
        return count > 1 ? t("statusbar.sync.syncedMany", { count }) : t("statusbar.sync.synced");
    }
  })();
  return (
    <button
      type="button"
      className="luna-workspace__statusbar-sync-btn"
      data-attention={SYNC_ATTENTION.has(sync.kind) || undefined}
      title={t("statusbar.sync.title")}
      onClick={() => {
        goTo("settings://account");
      }}
    >
      {label}
    </button>
  );
}

/** 状态栏铃铛：未读通知数只在这里表达（PRD-07-002b），点它打开通知抽屉。 */
export function StatusBarNotifications() {
  const { t } = useTranslation("workspace");
  const unread = useNotificationUnread();
  return (
    <button
      type="button"
      className="luna-workspace__statusbar-notification-btn"
      data-unread={unread > 0}
      // 它打开的是一扇模态抽屉，读屏该在按下之前就知道（PRD-15-064）。
      // 刻意**不给** `aria-expanded`：抽屉的开合状态不在这里，硬把它拎过来会多出
      // 一份必然与宿主分叉的镜像；而"再点一次关掉"由遮罩承担，不经这颗按钮。
      aria-haspopup="dialog"
      title={t("statusbar.notifications", { count: unread })}
      onClick={() => {
        goTo("notifications://");
      }}
    >
      <BellIcon size={13} />
      {unread > 0 && <span>{unread}</span>}
    </button>
  );
}
