/**
 * 说话人指派菜单（PRD-04-056，Otter / Descript / Sonix 对标）。
 *
 * 自动细分必然有错判，人工兜底的核心动作是「把这条（或这一轮 / 全篇）改判给某人」。
 * 菜单把**目标**（点谁就是谁）与**范围**（仅此条 / 及后续 / 全部）拆成两个维度：范围是
 * 一次选择后保持的模式（连续校对时不必反复设置），目标一点即执行并关闭——与 macOS
 * 上下文菜单「选项即动作」的直觉一致。
 *
 * 纯呈现组件：不发命令、不改数据，动作全部回调给回放视图（ADR-003 前端零业务，
 * 归属算法在 Rust `luna_core::speaker`）。复用文件树 `.luna-context-menu` 样式基座。
 *
 * **它此前自称 `role="menu"`，而它不是菜单**：菜单是"一列条目，选一条就走"；这里有
 * 单选名册、一个**选完要留着**的范围段控、以及两个动作——三种控件混在一起。按菜单声明，
 * 读屏就会承诺方向键在条目间移动，而这里根本没有方向键（只接了 Esc）。混合控件的浮层
 * 本来就是**对话框**：改用 `role="dialog"` + 批次 2 的 `useDialogBehavior`（Esc、焦点圈闭、
 * 关闭后焦点归位一次写好），Tab 键就按控件顺序自然走，不必再造一套键盘模型。
 */

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import type { SpeakerScopeDto, SpeakerStatDto } from "../../ipc/bindings";
import { CheckIcon, PencilIcon, PlusIcon } from "../../components/ui/Icon";
import { cssVariables } from "../../lib/css-variables";
import { speakerSlot } from "./speaker-view";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";

export interface SpeakerMenuProps {
  /** fixed 坐标（调用方已按视口 clamp）。 */
  anchor: { x: number; y: number };
  /** 名册（首次出现序 = 配色槽位序）。 */
  roster: SpeakerStatDto[];
  /** 当前段的说话人（`null` = 尚无标签）。 */
  current: string | null;
  /** 「新建说话人」将使用的 id（后端派生）。 */
  nextSpeakerId: string;
  /** speaker id → 显示名。 */
  displayName: (id: string) => string;
  /** 指派（目标 + 范围）。 */
  onAssign: (speaker: string, scope: SpeakerScopeDto) => void;
  /** 重命名当前说话人（就地输入交由回放视图承载）。 */
  onRename: (speaker: string) => void;
  onClose: () => void;
  /** 是否提示「一条里两个人说话怎么办」（常态列表下给出通往校对模式拆分的指路）。 */
  splitTip?: boolean;
}

const SCOPES: SpeakerScopeDto[] = ["single", "following", "all"];

export function SpeakerMenu({
  anchor,
  roster,
  current,
  nextSpeakerId,
  displayName,
  onAssign,
  onRename,
  onClose,
  splitTip = false,
}: SpeakerMenuProps) {
  const { t } = useTranslation("playback");
  const [scope, setScope] = useState<SpeakerScopeDto>("single");

  // Esc、焦点圈闭与关闭后焦点归位由共享 hook 负责（批次 2）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose });

  // 点浮层之外即关闭：这是浮层自己的手势（面板内 `onMouseDown` 已阻止冒泡），
  // 共享 hook 只管键盘那一半，故留在这里。
  useEffect(() => {
    const close = () => onClose();
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [onClose]);

  const assign = (speaker: string) => {
    onClose();
    onAssign(speaker, scope);
  };

  // portal 到 body：它是 fixed 定位的菜单，留在回放视图的层叠上下文里会被常驻画面层盖住。
  return createPortal(
    <div
      ref={dialogRef}
      className="luna-context-menu luna-speaker-menu"
      role="dialog"
      aria-label={t("speaker.menuTitle")}
      style={cssVariables({
        "--speaker-menu-x": `${anchor.x}px`,
        "--speaker-menu-y": `${anchor.y}px`,
      })}
      onMouseDown={(event) => event.stopPropagation()}
    >
      <p className="luna-speaker-menu__title">{t("speaker.menuTitle")}</p>
      {roster.map((stat) => (
        <button
          key={stat.id}
          type="button"
          aria-current={stat.id === current || undefined}
          data-speaker-slot={speakerSlot(stat.id, roster)}
          onClick={() => assign(stat.id)}
        >
          <span className="luna-context-menu__checkslot" aria-hidden="true">
            {stat.id === current ? <CheckIcon size={12} /> : null}
          </span>
          <span className="luna-speaker-menu__dot" aria-hidden="true" />
          <span className="luna-speaker-menu__name">{displayName(stat.id)}</span>
          <small className="luna-speaker-menu__count">
            {t("speaker.segmentCount", { count: stat.segment_count })}
          </small>
        </button>
      ))}
      <button type="button" onClick={() => assign(nextSpeakerId)}>
        <span className="luna-context-menu__checkslot" aria-hidden="true">
          <PlusIcon size={12} />
        </span>
        <span className="luna-speaker-menu__name">
          {t("speaker.newSpeaker", { name: displayName(nextSpeakerId) })}
        </span>
      </button>
      <div className="luna-context-menu__separator" role="separator" />
      <p className="luna-speaker-menu__title">{t("speaker.scopeTitle")}</p>
      <div className="luna-speaker-menu__scopes" role="group" aria-label={t("speaker.scopeTitle")}>
        {SCOPES.map((option) => (
          <button
            key={option}
            type="button"
            className="luna-speaker-menu__scope"
            aria-pressed={scope === option}
            title={t(`speaker.scopeHint.${option}`)}
            onClick={() => setScope(option)}
          >
            {t(`speaker.scope.${option}`)}
          </button>
        ))}
      </div>
      {current ? (
        <>
          <div className="luna-context-menu__separator" role="separator" />
          <button
            type="button"
            onClick={() => {
              onClose();
              onRename(current);
            }}
          >
            <span className="luna-context-menu__checkslot" aria-hidden="true">
              <PencilIcon size={12} />
            </span>
            <span>{t("speaker.renameAction", { name: displayName(current) })}</span>
          </button>
        </>
      ) : null}
      {splitTip ? <p className="luna-speaker-menu__tip">{t("speaker.splitTip")}</p> : null}
    </div>,
    document.body,
  );
}
