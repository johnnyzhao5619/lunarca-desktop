/**
 * 捕获胶囊（PRD-06-058 B / PRD-07-066：语音优先，文字为次）。
 *
 * 不问标题、不问分类、不问成熟度（GTD 的 capture/clarify 分离）：一切决策推迟到处理
 * 阶段。落盘形态（目录、文件名、`type: quick-note`）全由 Rust 决定，本组件只交出一段
 * 文本并消费返回的真实路径（ADR-003）。
 *
 * 两个入口、一个落点：麦克风按钮是主动作，唤起既有语音速记 HUD 且本次落点预设为「存为
 * 速记」（PRD-02-020），这里不新写采集逻辑；文字是次动作。
 *
 * **文字捕获按需展开，不常驻**（2026-10-08 修订，PRD-06-058 B）：此前侧栏常驻一个输入框，
 * 主区又是一块能直接写的新速记画布——同屏两个争夺注意的输入区，右边还要用一句"想随手记
 * 就用左上角的框"去解释另一个。现在主区画布是默认的写字处；侧栏只留「快速捕获」这一个
 * 动作，点它（或命令面板 / 「添加内容」/ 文字速记入口）才展开 `CaptureInput`，且同屏只挂
 * 一个。两种键盘语义不混：这里回车即存、⌘/Ctrl+回车存并打开、Shift+回车换行；正文画布
 * 的回车仍是换行。
 *
 * 收起只在**框里没字**时发生（Esc 或「收起」）：有字时收起会静默丢掉用户仅有的那一份。
 *
 * 文字框本体不住在这里：键盘契约与提交按钮归 `CaptureInput`，那份实现同时服务全局文字
 * 速记面板（PRD-06-058 B″）。本组件只负责这一格的编排、展开与焦点交接。
 */

import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { CloseIcon, MicIcon, PencilIcon } from "../../components/ui/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { CaptureStartButton } from "../../components/ui/CaptureStartButton";
import { CaptureInput } from "./CaptureInput";

/** 键盘教学脚注的 id。同屏只有一个捕获胶囊，常量比 `useId` 更容易在 DOM 里读出接线。 */
const HINT_ID = "luna-capture-hint";

export interface CaptureBoxProps {
  /** 落盘（`Enter`）；`open` = 落盘后打开编辑器（`⌘/Ctrl+Enter`）。返回是否成功。 */
  onCapture: (text: string, options: { open: boolean }) => Promise<boolean>;
  /** 唤起既有语音入口（HUD 原路不动，本组件不新写采集逻辑）。 */
  onVoiceCapture: () => void;
  /**
   * 语音输入的全局快捷键（已格式化）；空串 = 未设置，按钮只显名字。
   *
   * 挂在按钮上而不是另起一行脚注：这条提示的价值是「在别的应用里也能按」，与空态要讲
   * 的事不同，挤在一起谁也讲不清。
   */
  voiceShortcut: string;
  /**
   * 「展开文字捕获并聚焦」的请求序号（宿主每次 +1）。命令面板 / 「添加内容」/ 文字速记
   * 入口都走它——用户已经站在「速记」时再点一次，组件不会重挂载，`autoFocus` 不会再触发，
   * 故聚焦必须由这条显式请求驱动。`0` = 从未请求。
   */
  openRequest?: number;
}

export function CaptureBox({
  onCapture,
  onVoiceCapture,
  voiceShortcut,
  openRequest = 0,
}: CaptureBoxProps) {
  const { t } = useTranslation("workspace");
  const [open, setOpen] = useState(openRequest > 0);
  const [hasText, setHasText] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const toggleRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (openRequest === 0) {
      return;
    }
    setOpen(true);
    // 一帧之后输入框才挂上（刚从收起态展开时）。
    const frame = window.requestAnimationFrame(() => inputRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [openRequest]);

  function collapse() {
    if (hasText) {
      return;
    }
    setOpen(false);
    window.requestAnimationFrame(() => toggleRef.current?.focus());
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    // 输入法组字中的 Esc 是取消候选，不是收起。
    if (event.key !== "Escape" || event.nativeEvent.isComposing) {
      return;
    }
    if (!hasText) {
      event.preventDefault();
      collapse();
    }
  }

  return (
    <div className="luna-inbox__capture">
      {/* 主动作：语音。这一格里唯一一颗实色主按钮（PRD-07-067 ②）。
          形态与接线都走 `CaptureStartButton`——录音 / 转写 / 对话 / 速记四条采集侧栏的
          主按钮是同一种东西，包括那枚快捷键提示。 */}
      <CaptureStartButton
        icon={<MicIcon size={16} />}
        label={t("capture.startVoiceInput")}
        shortcut={voiceShortcut}
        onClick={onVoiceCapture}
      />
      {open ? (
        <div className="luna-inbox__capture-text" onKeyDown={onKeyDown}>
          <CaptureInput
            onCapture={onCapture}
            hintId={HINT_ID}
            inputRef={inputRef}
            autoFocus
            onValueChange={(value) => setHasText(value.trim() !== "")}
          />
          <IconButton
            className="luna-inbox__capture-close"
            label={t("draft.captureCollapse")}
            icon={<CloseIcon size={12} />}
            density="sm"
            disabled={hasText}
            onClick={collapse}
          />
        </div>
      ) : (
        <button
          ref={toggleRef}
          type="button"
          className="luna-btn luna-btn--secondary luna-inbox__capture-toggle"
          aria-expanded={false}
          onClick={() => {
            setOpen(true);
            window.requestAnimationFrame(() => inputRef.current?.focus());
          }}
        >
          <PencilIcon size={14} />
          <span>{t("draft.captureOpen")}</span>
        </button>
      )}
    </div>
  );
}
