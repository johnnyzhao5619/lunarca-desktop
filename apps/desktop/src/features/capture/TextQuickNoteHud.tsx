/**
 * 全局文字速记面板（`text-quick-note` 非激活 NSPanel，PRD-06-058 B″）。
 *
 * 它解决的问题：语音速记有四条路（全局热键 / 托盘 / 添加内容 / 侧栏主按钮），文字速记在
 * 这扇窗出现之前只有应用内两条，且都要求主窗口在前台。捕获够不着，后面那套整理能力就是
 * 空转。
 *
 * 不夺焦（ADR-015）：非激活面板 + 可输入焦点模式（Rust 侧预热时转换）。用户在别的应用里
 * 选着一段文字按下热键，那段选区必须还在。
 *
 * 不失焦收起：点一下别的应用就把窗连同用户敲了一半的字带走，是这扇窗唯一不可接受的失败。
 * 收起只有 Esc 与关闭键两条显式路径。
 *
 * 键盘契约不在这里实现：文本框与它的规则归 `CaptureInput`，侧栏胶囊用的是同一份。
 *
 * 可用性判定不自己写：库没打开时能不能捕获，答案来自 Rust 的 `create_action_states`，与
 * 「添加内容」里那一行同一条通路、同一句文案（ADR-003）。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { HudSurface } from "../../components/ui/HudSurface";
import { HudHeader } from "../../components/ui/HudHeader";
import { HudReceipt, type HudReceiptValue } from "../../components/ui/HudReceipt";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon, StickyNoteIcon } from "../../components/ui/Icon";
import { useWindowDrag } from "../../hooks/use-window-drag";
import { usePanelTextEditing } from "../../hooks/use-panel-text-editing";
import { commands, type WorkspaceMetricEntrySurfaceDto } from "../../ipc/bindings";
import { recordCreateEntry } from "../workspace/use-workspace-metrics";
import { useCreateActionStates } from "./AddContentPopover";
import {
  CREATE_ACTION_REASON_KEYS,
  CREATE_ACTION_REPAIR_LABEL_KEYS,
  runCreateActionRepair,
  type CreateActionContext,
} from "./create-actions";
import { CaptureInput } from "./CaptureInput";
import { goTo } from "../../lib/navigate";
import { vaultErrorText } from "../../lib/vault-error";

/** 脚注 id：同屏只有一个捕获框，取常量（同 `CaptureBox` 的理由）。 */
const HINT_ID = "luna-quick-capture-hint";

declare global {
  interface Window {
    /** 显示桥（`show_text_quick_note_panel` 经 eval 调用）：聚焦输入框并记下入口面。 */
    __lunaShowQuickNote?: (origin: string) => void;
  }
}

/**
 * Rust 带过来的入口面 → 指标枚举。
 *
 * 不认识的值退回 `shortcut` 而不是抛：它走的是 `eval` 这条没有类型的通道，把一次指标归
 * 错档，代价远小于让一个已经浮出来的捕获窗因为一句遥测而白屏。
 */
function entrySurface(origin: string): WorkspaceMetricEntrySurfaceDto {
  return origin === "tray" ? "tray" : "shortcut";
}

export function TextQuickNoteHud() {
  const { t } = useTranslation("workspace");
  const { t: tCommon } = useTranslation("common");
  const { onPointerDown: handleDrag } = useWindowDrag();
  const editing = usePanelTextEditing();
  const inputRef = useRef<HTMLTextAreaElement>(null);
  /** 本次唤起以来存下的条数（回执）；每次重新唤起归零。 */
  const [savedCount, setSavedCount] = useState(0);
  /** 最近存下的那一篇（回执上「打开」的去处）。 */
  const [lastSavedPath, setLastSavedPath] = useState<string | null>(null);
  /** 存下回执此刻是否在场：淡出后清掉，下一次存下再出现。 */
  const [receiptVisible, setReceiptVisible] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  /** 草稿写成了多行：此时才长出卡片档（ADR-137 V7：敲第一个字不跃迁）。 */
  const [multiline, setMultiline] = useState(false);
  /** 这一次是从哪儿唤起的——落盘成功后按它归档指标。 */
  const originRef = useRef<WorkspaceMetricEntrySurfaceDto>("shortcut");
  /** 唤起那一刻（`performance.now()`）：入口耗时的起点。 */
  const startedAtRef = useRef(0);
  const { snapshot, refresh } = useCreateActionStates();

  const close = useCallback(() => {
    void commands.textQuickNoteClose();
  }, []);

  // 显示桥：Rust 每次 show 之后调它。聚焦必须在这里做——面板是预热出来的，组件早在用户
  // 第一次按热键之前就挂载完了，`autoFocus` 不会再触发。
  useEffect(() => {
    window.__lunaShowQuickNote = (origin: string) => {
      originRef.current = entrySurface(origin);
      startedAtRef.current = performance.now();
      setSavedCount(0);
      setLastSavedPath(null);
      setReceiptVisible(false);
      setSaveError(null);
      // 每次唤起重查可用性：库可能在两次唤起之间被关掉或换掉，而非激活面板拿不到窗口
      // focus，订阅那一路补不上这一类变化。
      refresh();
      inputRef.current?.focus();
    };
    return () => {
      delete window.__lunaShowQuickNote;
    };
  }, [refresh]);

  const entry = snapshot?.actions.find((action) => action.id === "text_quick_note");
  const blocked = entry !== undefined && entry.state.kind !== "available";
  const reason = entry !== undefined && entry.state.kind !== "available" ? entry.state.reason : null;
  const repair = entry?.state.kind === "needs_attention" ? entry.state.repair : null;

  /**
   * 修复动作的上下文。这扇窗只可能用到「去设置」与「去授权」两条，但仍给全五条：
   * `CreateActionContext` 是穷尽的，缺一条就要靠类型断言才编译得过，而断言正是「少一格
   * 不会有任何东西变红」的开始。用不到的三条如实返回 `rejected`——不是什么都没发生，
   * 是这里做不了。
   */
  const repairContext: CreateActionContext = {
    captureText: () => Promise.resolve("rejected"),
    captureVoice: () => Promise.resolve("rejected"),
    createNote: () => Promise.resolve("rejected"),
    startRecording: () => Promise.resolve("rejected"),
    importMaterial: () => Promise.resolve("rejected"),
  };

  async function capture(text: string, options: { open: boolean }): Promise<boolean> {
    setSaveError(null);
    try {
      const result = await commands.createNoteFromText(text, "drafts");
      if (result.status !== "ok") {
        // 落盘失败：`CaptureInput` 据此不清空，并就地说原因。顺带重查可用性——最常见的
        // 失败就是库在这中间被关掉了，重查会把提交禁掉并给出「去解决」。
        setSaveError(vaultErrorText(result.error));
        recordCreateEntry("text_quick_note", originRef.current, "failed", startedAtRef.current);
        refresh();
        return false;
      }
      recordCreateEntry("text_quick_note", originRef.current, "arrived", startedAtRef.current);
      setSavedCount((count) => count + 1);
      setLastSavedPath(result.data.path);
      setReceiptVisible(true);
      if (options.open) {
        // ⌘/Ctrl+回车：落盘 + 把主窗带到前台并选中这一篇。`open_workspace_route` 自己会
        // show/focus 主窗，这里不另拼一条唤起路径。
        goTo(`quick-notes://${result.data.path}`);
        close();
      }
      return true;
    } catch (error) {
      setSaveError(vaultErrorText(error instanceof Error ? error.message : String(error)));
      recordCreateEntry("text_quick_note", originRef.current, "failed", startedAtRef.current);
      refresh();
      return false;
    }
  }

  // 回执线（ADR-137 V5）：不可用与没存上常驻在行下；存下的回执在行内、会自己淡出。
  const problem: HudReceiptValue | null = blocked
    ? {
        kind: "error",
        message: reason ? t(CREATE_ACTION_REASON_KEYS[reason]) : t("capture.hud.saveFailed"),
        action: repair
          ? {
              label: t(CREATE_ACTION_REPAIR_LABEL_KEYS[repair.kind]),
              onSelect: () => {
                recordCreateEntry("text_quick_note", originRef.current, "rejected", startedAtRef.current);
                if (runCreateActionRepair(repair, repairContext)) {
                  close();
                }
              },
            }
          : undefined,
      }
    : saveError !== null
      ? { kind: "error", message: t("capture.hud.saveFailed"), detail: saveError }
      : null;
  const saved: HudReceiptValue | null =
    receiptVisible && savedCount > 0
      ? {
          kind: "success",
          message: t("capture.hud.savedCount", { count: savedCount }),
          action: lastSavedPath
            ? {
                label: t("capture.hud.openLast"),
                onSelect: () => {
                  goTo(`quick-notes://${lastSavedPath}`);
                  close();
                },
              }
            : undefined,
        }
      : null;

  return (
    <HudSurface
      className="luna-quick-capture"
      aria-label={t("capture.hud.title")}
      // `dragRoot={false}`：整面可拖会让正文不可选中，而这扇窗的全部内容就是一个可编辑
      // 文本框。拖动归状态行的空白处（同快捷对话）。
      dragRoot={false}
      autoGrow
      tier={problem !== null || multiline ? "card" : "pill"}
      onDismiss={close}
    >
      {/* 启动器式一行（同 Raycast / Spotlight）：身份图标 · 输入框 · 回执 · 关闭。
          不可用时输入框保留、只禁提交：草稿留在原处，修好之后直接存（裁决修订见 ADR-119）。 */}
      <HudHeader
        className="luna-quick-capture__header"
        onPointerDown={handleDrag}
        data-draggable="true"
        icon={<StickyNoteIcon size={15} />}
        title={t("capture.hud.title")}
        // `uplink` 传 null 而不是省略（ADR-038 ④ / ADR-111 ⑤⑴）：落盘全程不出设备，
        // 这里是如实的「没有联网指示」，不是忘了接。
        uplink={null}
        lane={
          <CaptureInput
            onCapture={capture}
            hintId={HINT_ID}
            hint="screen-reader"
            inputRef={inputRef}
            disabled={blocked}
            onValueChange={(value) => {
              setMultiline(value.includes("\n"));
              if (value.length > 0) setReceiptVisible(false);
            }}
            onEditingChange={(active) => (active ? editing.onFocus() : editing.onBlur())}
          />
        }
        actions={
          <>
            <HudReceipt receipt={saved} onExpire={() => setReceiptVisible(false)} />
            <IconButton
              label={tCommon("action.close")}
              icon={<CloseIcon size={14} />}
              density="hud"
              onClick={close}
            />
          </>
        }
      />
      <HudReceipt receipt={problem} />
    </HudSurface>
  );
}
