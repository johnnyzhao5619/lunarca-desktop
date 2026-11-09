import { useEffect, useState } from "react";
import { useEditorExitGuard } from "../features/editor/use-editor-exit-guard";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { commands, events, type AppBootstrapDto, type PresetInfo } from "../ipc/bindings";
import { PopupTranslateView } from "../features/popup-translate/PopupTranslateView";
import { ScreenshotOverlay } from "../features/popup-translate/ScreenshotOverlay";
import { DesktopLensTrigger, DesktopLensView } from "../features/popup-translate/DesktopLensView";
import { VoiceInputHud } from "../features/voice-input/VoiceInputHud";
import { TextQuickNoteHud } from "../features/capture/TextQuickNoteHud";
import { DesktopNoteWindow } from "../features/capture/DesktopNoteWindow";
import { RecordingHud } from "../features/recording/RecordingHud";
import { CaptionPanel } from "../features/caption/CaptionPanel";
import { TeleprompterPanel } from "../features/teleprompter/TeleprompterPanel";
import { CaptionTranslationWindow } from "../features/caption/CaptionTranslationWindow";
import { NoteWindow } from "../features/editor/NoteWindow";
import { NotePrintView } from "../features/editor/NotePrintView";
import { SecondaryVaultWindow } from "../features/vault/SecondaryVaultWindow";
import { WorkspaceApp } from "./WorkspaceApp";
import { KnowledgeReceiptHost } from "../features/knowledge/KnowledgeReceiptHost";
import { CitationOutcomeHost } from "../features/chat/CitationOutcomeHost";
import { ChatPanel } from "../features/chat/ChatPanel";
import { NotificationDrawerHost } from "../features/notifications/NotificationDrawerHost";
import { ExportOptionsHost } from "../features/transcribe/ExportOptionsHost";
import { NotificationHost } from "../features/notifications/NotificationHost";
import { OnboardingWizard } from "../features/onboarding/OnboardingWizard";
import { FirstSyncPrompt } from "../features/account/FirstSyncPrompt";
import { useOnboarding } from "../features/onboarding/use-onboarding";
import { SetupStatusProvider, useSetupStatus } from "../features/onboarding/use-setup-status";
import { useAppearance } from "../theme/use-appearance";
import { OPEN_ONBOARDING_EVENT } from "../lib/app-events";
import { launchOverviewEnabled } from "../features/dashboard/launch-overview";

export function App() {
  useEditorExitGuard();
  const windowLabel = getCurrentWebviewWindow().label;
  // 启动快照（PRD-14-066）：外观、引导门控、库状态与全量设置**一次**取回，供首帧用；
  // 此前这四份数据在启动期各自跨进程读了一遍。任何一扇窗都要外观，故每扇窗各取一次。
  const [bootstrap, setBootstrap] = useState<AppBootstrapDto | null>(null);
  const [bootstrapFailed, setBootstrapFailed] = useState(false);
  useEffect(() => {
    void commands.appBootstrap().then((result) => {
      if (result.status === "ok") {
        setBootstrap(result.data);
      } else {
        // 快照拿不到（锁中毒一类的异常）：各 hook 退回各自读一遍，应用照常起来。
        setBootstrapFailed(true);
      }
    });
  }, []);
  useAppearance(bootstrap?.settings ?? null, bootstrapFailed);
  // 装机就绪状态的**唯一**订阅（PRD-14-063）：引导向导、「完成设置」横幅、启动概览门控
  // 都消费这一份；只有主窗需要它。
  const setupStatus = useSetupStatus(windowLabel === "main");
  const onboarding = useOnboarding(bootstrap?.onboarding ?? null, bootstrapFailed, setupStatus);
  // 库世代：每次切库自增，驱动工作区整体重挂载（见下方 key 注释）。
  const [vaultEpoch, setVaultEpoch] = useState(0);
  // Rust 换了库或交出了库：换了账户跟着换（ADR-151 修订 2026-10-15「续二」），或启动时还在 iCloud
  // 上的库取回后自动打开。这次切库不经界面的切换控制器，同样整体重挂载工作区。只有主窗承载工作区。
  useEffect(() => {
    if (windowLabel !== "main") return;
    const unlisten = events.vaultSwitchedByApp.listen(() => setVaultEpoch((epoch) => epoch + 1));
    return () => void unlisten.then((dispose) => dispose());
  }, [windowLabel]);
  // 引导覆盖层（PRD-14-060 重入）：设置页「重新运行引导」与「完成设置」横幅都经同一个事件
  // 打开；引导压在工作区之上而不是替换它——用户正在编辑的东西不该因为想看一眼引导而丢失。
  const [overlayOpen, setOverlayOpen] = useState(false);
  const openOnboarding = onboarding.open;
  useEffect(() => {
    const handler = (event: Event) => {
      const step = event instanceof CustomEvent ? (event.detail as string | undefined) : undefined;
      void openOnboarding(step).then(() => setOverlayOpen(true));
    };
    window.addEventListener(OPEN_ONBOARDING_EVENT, handler);
    return () => window.removeEventListener(OPEN_ONBOARDING_EVENT, handler);
  }, [openOnboarding]);
  const [presets, setPresets] = useState<PresetInfo[]>([]);
  const isPopup = windowLabel === "popup-translate";
  // 截屏框选层为多屏面板池：`screenshot-overlay-<显示器序号>`（与 Rust SCREENSHOT_OVERLAY_PREFIX 同源）。
  const isScreenshotOverlay = windowLabel.startsWith("screenshot-overlay-");
  const isDesktopLens = windowLabel === "desktop-lens";
  const isDesktopLensTrigger = windowLabel === "desktop-lens-trigger";
  const isVoiceInputHud = windowLabel === "voice-input-hud";
  const isQuickChat = windowLabel === "quick-chat";
  // 全局文字速记面板（PRD-06-058 B″）；标签与 Rust 的 `TEXT_QUICK_NOTE_WINDOW` 同源。
  const isTextQuickNote = windowLabel === "text-quick-note";
  // 桌面便签窗口池：`desktop-note-<n>`（与 Rust 的 DESKTOP_NOTE_WINDOW_PREFIX 同源）。
  const isDesktopNote = windowLabel.startsWith("desktop-note-");
  const isRecordingHud = windowLabel === "recording-hud";
  const isLiveCaption = windowLabel === "live-caption";
  const isLiveCaptionTranslation = windowLabel === "live-caption-translation";
  // 提词器（ADR-162 ⑤）：标签与 Rust 的 `TELEPROMPTER_LABEL` 同源。
  const isTeleprompter = windowLabel === "teleprompter";
  // 弹出编辑器独立窗口池：`note-editor-<序号>`（与 Rust NOTE_EDITOR_WINDOW_PREFIX 同源）。
  const isNoteWindow = windowLabel.startsWith("note-editor-");
  // 打印窗口 `note-print-<序号>`（PDF 导出，PRD-11-011）：同一份阅读视图 + 系统打印面板。
  const isPrintWindow = windowLabel.startsWith("note-print-");
  // 副库独立窗口池：`vault-window-<序号>`（与 Rust VAULT_WINDOW_PREFIX 同源，PRD-06-050）。
  const isVaultWindow = windowLabel.startsWith("vault-window-");

  useEffect(() => {
    if (isPopup) {
      void commands.listPopupPresets().then(setPresets);
    }
  }, [isPopup]);

  // 冷启动里程碑「首帧」（TDD-13 §6）：主窗读回引导状态、决定画哪一面之后即算。
  // 只有主窗打；HUD / 弹窗各有各的预热，不是冷启动的一部分。
  useEffect(() => {
    if (windowLabel === "main" && !onboarding.loading) {
      void commands.startupMark("first_frame");
    }
  }, [windowLabel, onboarding.loading]);

  if (isPopup) {
    return <PopupTranslateView presets={presets} />;
  }

  if (isScreenshotOverlay) {
    return <ScreenshotOverlay />;
  }

  if (isDesktopLens) {
    return <DesktopLensView />;
  }

  if (isDesktopLensTrigger) {
    return <DesktopLensTrigger />;
  }

  if (isVoiceInputHud) {
    return <VoiceInputHud />;
  }

  if (isQuickChat) {
    return <ChatPanel />;
  }

  if (isTextQuickNote) {
    return <TextQuickNoteHud />;
  }

  if (isDesktopNote) {
    return <DesktopNoteWindow />;
  }

  if (isRecordingHud) {
    return <RecordingHud />;
  }

  if (isLiveCaption) {
    return <CaptionPanel />;
  }

  if (isLiveCaptionTranslation) {
    return <CaptionTranslationWindow />;
  }

  if (isTeleprompter) {
    return <TeleprompterPanel />;
  }

  if (isPrintWindow) {
    return <NotePrintView />;
  }
  if (isNoteWindow) {
    return <NoteWindow />;
  }

  if (isVaultWindow) {
    return <SecondaryVaultWindow />;
  }

  // 首启状态尚未读回前不能先挂载整套工作区：否则工作区所有查询会在这一帧提前并发，
  // 随后又被引导页整体卸载，既制造无意义 IPC，也会在慢盘上闪出错误空态。
  if (onboarding.loading) {
    return <NotificationHost />;
  }

  if (onboarding.shouldShow) {
    return (
      <>
        <NotificationHost />
        <OnboardingWizard controller={onboarding} />
      </>
    );
  }

  return (
    <SetupStatusProvider value={setupStatus}>
      {/* key = 库世代：切库完成即整体重挂载工作区（PRD-06-049 / PRD-07-064）——布局按
          新库快照重新恢复，标签页/对话/搜索/图谱等一切前端态一次归零，不逐视图清理。 */}
      <WorkspaceApp
        key={vaultEpoch}
        // 首次挂载吃启动快照；切库重挂载时快照已过期，交给工作区自己重读。
        //
        // **刚在这次会话里走完引导的那条路同样不能吃快照**：快照是 App 挂载时取的，
        // 那时还没有库，而用户接着在引导里绑好了库。拿它开工作区，主区会说「尚未选择
        // Vault」——同一屏上状态栏正显示着库名、顶部横幅还把 Vault 算作已完成
        // （真机 A1）。索引扫描完成事件本可纠正它，但新库是空的、那个事件在工作区
        // 挂载前就发完了，于是这个假状态会一直留到下次重启。
        // 快照存在的意义是省掉"已装机启动"那条快路上的一次 IPC；刚花了几分钟走完引导的
        // 人不在那条路上。
        initialVault={
          vaultEpoch === 0 && bootstrap?.onboarding.completed === true
            ? (bootstrap?.vault ?? null)
            : null
        }
        launchOverview={launchOverviewEnabled(bootstrap)}
        onVaultSwitched={() => setVaultEpoch((epoch) => epoch + 1)}
      />
      <KnowledgeReceiptHost />
      <CitationOutcomeHost />
      <NotificationHost />
      <FirstSyncPrompt />
      {/* 通知抽屉与工作区**同层**（ADR-087 ② / PRD-15-064）：它是覆盖在当前工作上下文
          之上的历史宿主，不是工作区的一页——故它不随切库重挂载（`key` 只给工作区），
          也不依赖任何一级导航目的地。铃铛与 `notifications://` 打开的都是它。 */}
      <NotificationDrawerHost />
      {/* 「导出选项」对话框宿主（PRD-11-004）：三个导出入口共用同一张，不各挂一份。 */}
      <ExportOptionsHost />
      {overlayOpen ? (
        <OnboardingWizard controller={onboarding} overlay onClose={() => setOverlayOpen(false)} />
      ) : null}
    </SetupStatusProvider>
  );
}
