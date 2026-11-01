/**
 * 捕获输入（`.luna-capture-field`）——「回车即落袋」那个控件的唯一实现。
 *
 * 两个消费面：「速记」侧栏的捕获胶囊（`CaptureBox`）与全局文字速记面板
 * （`TextQuickNoteHud`，PRD-06-058 B / B″）。
 *
 * 抽出来而不是各写一遍，是因为这个控件的全部产品面就是它的键盘契约，而契约里那条
 * 「输入法上屏的回车不算提交」最容易犯、也最难被英文测试发现。
 *
 * 键盘契约（两个消费面逐字相同）：
 * - `Enter` 存下并留在框里（清空 + 焦点不走），连续捕获零决策；
 * - `Shift+Enter` 换行，交给 textarea 原生行为；
 * - `⌘/Ctrl+Enter` 存下并打开（`open: true`，「打开」是什么由调用方定）；
 * - 输入法上屏那一下不算提交（判据在 `capture-keys.ts` → `lib/ime-composition.ts`）；
 * - 没存成就不清空——用户刚敲的那段字是他仅有的一份。
 *
 * `preventDefault()` 只在真的要提交时才调（2026-09-09 真机修复）：判「这一下是什么」与
 * 「要不要拦下它」必须是同一个判断，否则被判为让给输入法的那一下也已经被吃掉了。
 *
 * 文案不走 props：两个消费面是同一个控件，说的必须是同一句话。面板自己那句（Esc 关闭、
 * 本次已存几条）属于面板，写在面板的脚注里。
 *
 * 高度随内容长（封顶数行）走纯 CSS 的镜像格子（`data-value` + `::after`），不在 JS 里量
 * `scrollHeight` 再写回 style——那是内联样式（TDD-15 §3.2 禁），且每次输入都要读一次布局。
 */

import {
  useRef,
  useState,
  type CompositionEvent,
  type KeyboardEvent,
  type RefObject,
} from "react";
import { useTranslation } from "react-i18next";
import { IconButton } from "../../components/ui/IconButton";
import { SendIcon } from "../../components/ui/Icon";
import { type CompositionState, NO_COMPOSITION } from "../../lib/ime-composition";
import { classifyCaptureKey } from "./capture-keys";

export interface CaptureInputProps {
  /** 落盘（`Enter`）；`open` = 落盘后打开（`⌘/Ctrl+Enter`）。返回是否成功。 */
  onCapture: (text: string, options: { open: boolean }) => Promise<boolean>;
  /**
   * 脚注 id（`aria-describedby` 的目标）。每个消费面给一个稳定常量——它们各在一扇窗里、
   * 同屏只有一个捕获框，常量比 `useId` 更容易在 DOM 里读出接线。
   */
  hintId: string;
  /** 挂载即聚焦（侧栏用）。面板走 `inputRef` 显式聚焦，不靠挂载副作用。 */
  autoFocus?: boolean;
  /**
   * 外部持有的输入框引用。面板被热键或托盘唤起时要显式把焦点交回来，而那时组件早已
   * 挂载，`autoFocus` 不会再触发。
   */
  inputRef?: RefObject<HTMLTextAreaElement>;
  /**
   * 进入 / 离开文本编辑态。非激活面板据此升降窗口层级，否则输入法候选窗被面板压住
   * （TDD-05 §5.1.1）；侧栏不传（主窗口不是面板，那个命令在平台层是 no-op）。
   */
  onEditingChange?: (editing: boolean) => void;
  /** 外部业务状态不可用时保留草稿，但禁止提交。 */
  disabled?: boolean;
  /** 草稿变化通知，用于宿主切换 HUD 内容档位。 */
  onValueChange?: (value: string) => void;
  /**
   * 键盘教学脚注的呈现：侧栏胶囊常显；启动器式的速记面板只有一行（ADR-137 V2），脚注
   * 只给读屏——契约仍经 `aria-describedby` 念到，占位符与发送钮已说明「回车即存」。
   */
  hint?: "visible" | "screen-reader";
}

export function CaptureInput({
  onCapture,
  hintId,
  autoFocus,
  inputRef,
  onEditingChange,
  disabled = false,
  onValueChange,
  hint = "visible",
}: CaptureInputProps) {
  const { t } = useTranslation("workspace");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const localRef = useRef<HTMLTextAreaElement>(null);
  const ref = inputRef ?? localRef;
  /**
   * 合成边界（`lib/ime-composition.ts` 的判据之一）。
   *
   * 存 ref 不存 state：它只在按键那一刻被读一次，而中文输入每敲一个字母都会走一遍组字
   * 事件，做成 state 就是每个字母重渲一次。
   */
  const compositionRef = useRef<CompositionState>({ ...NO_COMPOSITION });

  async function submit(open: boolean) {
    if (busy || disabled || value.trim() === "") {
      return;
    }
    setBusy(true);
    let saved = false;
    try {
      saved = await onCapture(value, { open });
    } finally {
      setBusy(false);
    }
    if (!saved) {
      // 没存成就不清空：用户刚敲的那段字是他仅有的一份。
      return;
    }
    setValue("");
    onValueChange?.("");
    if (!open) {
      // 存后焦点留在框里 = 下一条接着敲。
      ref.current?.focus();
    }
  }

  function onCompositionStart() {
    compositionRef.current = { active: true, endedAt: compositionRef.current.endedAt };
  }

  function onCompositionEnd(event: CompositionEvent<HTMLTextAreaElement>) {
    // 记下这次上屏发生在什么时候：WKWebView 里紧随其后的 keydown 就是同一次物理按键，
    // 而它自己身上已经没有任何「我属于输入法」的痕迹。
    compositionRef.current = { active: false, endedAt: event.timeStamp };
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    const action = classifyCaptureKey(
      {
        key: event.key,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        isComposing: event.nativeEvent.isComposing,
        keyCode: event.nativeEvent.keyCode,
        timeStamp: event.timeStamp,
      },
      compositionRef.current,
    );
    // `ignore` 与 `newline` 都必须原样放行：前者是输入法的上屏键（吃掉它候选词就上不了
    // 屏），后者是 textarea 的原生换行。
    if (action === "ignore" || action === "newline") {
      return;
    }
    event.preventDefault();
    void submit(action === "saveAndOpen");
  }

  return (
    <div className="luna-capture-field">
      {/* 镜像格子：`::after` 拿同一段文本占位，textarea 于是恰好长到内容那么高。 */}
      <div className="luna-capture-field__row">
        <div className="luna-capture-field__grow" data-value={value}>
          <textarea
            ref={ref}
            className="luna-input luna-capture-field__input"
            rows={1}
            value={value}
            autoFocus={autoFocus}
            disabled={busy || disabled}
            placeholder={t("draft.capturePlaceholder")}
            aria-label={t("draft.captureLabel")}
            aria-describedby={hintId}
            onChange={(event) => {
              setValue(event.target.value);
              onValueChange?.(event.target.value);
            }}
            onKeyDown={onKeyDown}
            onCompositionStart={onCompositionStart}
            onCompositionEnd={onCompositionEnd}
            onFocus={() => onEditingChange?.(true)}
            onBlur={() => onEditingChange?.(false)}
          />
        </div>
        <IconButton
          className="luna-capture-field__submit"
          label={t("draft.captureSubmit")}
          icon={<SendIcon size={14} />}
          density="sm"
          disabled={busy || disabled || value.trim() === ""}
          // 掐掉 mousedown 的默认焦点转移：不掐的话 textarea 先失焦，存完再 focus 回来会
          // 闪一下，且在 WKWebView 上会被那次默认行为勾销。
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => void submit(false)}
        />
      </div>
      {/*
        键盘教学脚注：静息显示短句、聚焦时换成完整三条（纯 CSS，无 JS 状态）。
        两个 span 恒在 DOM 里——`aria-describedby` 指向父节点，读屏念到的始终是完整契约。
      */}
      <p
        className={
          hint === "visible" ? "luna-capture-field__hint" : "luna-capture-field__hint luna-visually-hidden"
        }
        id={hintId}
      >
        <span className="luna-capture-field__hint-short">{t("draft.captureHintShort")}</span>
        <span className="luna-capture-field__hint-full">{t("draft.captureHint")}</span>
      </p>
    </div>
  );
}
