// AI 指令框（PRD-06-020，Notion "Ask AI" / Apple 写作工具对标）：一行输入 + 分组动作列表。
//
// 键盘优先：打开即聚焦输入框；↑↓ 在动作间移动，Enter 执行高亮的动作；输入了文字时 Enter
// **恒**发送这条指令（typed 的意图不被碰巧匹配的动作抢走），要跑匹配到的动作就先按 ↓。
// 输入即过滤列表（按标签子串）。带子菜单的动作（翻译 ▸ 语言、总结 ▸ 风格、改语气 ▸ 档）
// Enter / 点击后进入子层，Esc / ← / 「返回」回到上层。这里只摆放与转发：动作清单由宿主
// 按作用域给，执行位置 / 出域字数 / 上下文开关都是 Rust 事实的投影。

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ChevronLeftIcon, ChevronRightIcon, SendIcon, TrashIcon } from "../../components/ui/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { isImeKeyEvent } from "../../lib/ime-key-event";

export type ComposerGroup = "presets" | "edit" | "generate" | "understand" | "vault";

const GROUP_ORDER: readonly ComposerGroup[] = ["presets", "edit", "generate", "understand", "vault"];

export interface ComposerItem {
  id: string;
  group: ComposerGroup;
  label: string;
  icon: ReactNode;
  disabled?: boolean;
  /** 有子菜单的动作：Enter / 点击进入子层，子项才是真正执行的。 */
  children?: ComposerItem[];
  run?: () => void;
  /** 可删除的项（用户预设）：行尾一枚删除钮。 */
  onRemove?: () => void;
}

interface AiComposerProps {
  items: ComposerItem[];
  placeholder: string;
  /** 执行位置标记：`null` = 尚未查到（不渲染）。 */
  routeLabel: string | null;
  /** 云路径且动作会带上下文时给「只发选区」开关；`null` = 不渲染开关。 */
  contextToggle: { enabled: boolean; onToggle: () => void } | null;
  hint: string;
  /** 提交前预检提示（长文策略，PRD-06-025）或一次性状态；`null` = 不渲染。 */
  notice?: string | null;
  onAsk: (instruction: string) => void;
}

export function AiComposer({
  items,
  placeholder,
  routeLabel,
  contextToggle,
  hint,
  notice = null,
  onAsk,
}: AiComposerProps) {
  const { t } = useTranslation("editor");
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [draft, setDraft] = useState("");
  /** -1 = 高亮在输入框自身（Enter 发指令）；≥0 = 过滤后列表的下标。 */
  const [highlighted, setHighlighted] = useState(-1);
  /** 当前打开的子菜单（`null` = 顶层）。 */
  const [parent, setParent] = useState<ComposerItem | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const query = draft.trim().toLowerCase();
  const visible = parent?.children ?? items;
  const filtered = useMemo(
    () => (query ? visible.filter((item) => item.label.toLowerCase().includes(query)) : visible),
    [visible, query],
  );

  const grouped = useMemo(() => {
    const byGroup = new Map<ComposerGroup, { item: ComposerItem; index: number }[]>();
    filtered.forEach((item, index) => {
      const bucket = byGroup.get(item.group) ?? [];
      bucket.push({ item, index });
      byGroup.set(item.group, bucket);
    });
    return GROUP_ORDER.filter((group) => byGroup.has(group)).map((group) => ({
      group,
      entries: byGroup.get(group) ?? [],
    }));
  }, [filtered]);

  function activate(item: ComposerItem) {
    if (item.disabled) {
      return;
    }
    if (item.children && item.children.length > 0) {
      setParent(item);
      setHighlighted(0);
      setDraft("");
      return;
    }
    item.run?.();
  }

  function back() {
    setParent(null);
    setHighlighted(-1);
    setDraft("");
  }

  function submit() {
    if (highlighted >= 0 && highlighted < filtered.length) {
      activate(filtered[highlighted]);
      return;
    }
    const instruction = draft.trim();
    if (instruction) {
      setDraft("");
      onAsk(instruction);
    }
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    // 组字中的方向键在挑候选、回车在上屏（PRD-07-040 ①）。
    if (isImeKeyEvent(event)) {
      return;
    }
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setHighlighted((current) => (filtered.length === 0 ? -1 : (current + 1) % filtered.length));
        return;
      case "ArrowUp":
        event.preventDefault();
        setHighlighted((current) =>
          filtered.length === 0 ? -1 : current <= 0 ? filtered.length - 1 : current - 1,
        );
        return;
      case "ArrowLeft":
        if (parent && draft.length === 0) {
          event.preventDefault();
          back();
        }
        return;
      case "Escape":
        // 子层的 Esc 只回上层，不关面板（关面板是顶层 Esc 的事，由容器接手）。
        if (parent) {
          event.preventDefault();
          event.stopPropagation();
          back();
        }
        return;
      case "Enter":
        event.preventDefault();
        submit();
        return;
      default:
        return;
    }
  }

  return (
    <div className="luna-floating-menu__composer">
      <div className="luna-floating-menu__ask">
        <input
          ref={inputRef}
          type="text"
          className="luna-input"
          value={draft}
          placeholder={parent ? parent.label : placeholder}
          aria-label={t("floating.ask_label")}
          aria-activedescendant={
            highlighted >= 0 && filtered[highlighted]
              ? `luna-ai-item-${filtered[highlighted].id}`
              : undefined
          }
          onChange={(event) => {
            setDraft(event.target.value);
            setHighlighted(-1);
          }}
          onKeyDown={onKeyDown}
        />
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-floating-menu__send"
          disabled={!draft.trim()}
          aria-label={t("floating.action.send")}
          onClick={submit}
        >
          <SendIcon size={14} />
        </button>
      </div>
      {routeLabel || contextToggle ? (
        <div className="luna-floating-menu__meta">
          {routeLabel ? <span className="luna-floating-menu__route">{routeLabel}</span> : null}
          {contextToggle ? (
            <button
              type="button"
              className="luna-floating-menu__context-toggle"
              aria-pressed={contextToggle.enabled}
              title={t("floating.context.toggle_title")}
              onClick={contextToggle.onToggle}
            >
              {t(contextToggle.enabled ? "floating.context.on" : "floating.context.off")}
            </button>
          ) : null}
        </div>
      ) : null}
      {notice ? (
        <p className="luna-floating-menu__notice" role="status">
          {notice}
        </p>
      ) : null}
      {parent ? (
        <button type="button" className="luna-floating-menu__back" onClick={back}>
          <ChevronLeftIcon size={14} />
          <span>{t("floating.back")}</span>
          <span className="luna-floating-menu__back-title">{parent.label}</span>
        </button>
      ) : null}
      {filtered.length > 0 ? (
        <div className="luna-floating-menu__list" role="listbox" aria-label={t("floating.title")}>
          {grouped.map(({ group, entries }) => (
            <div key={group} className="luna-floating-menu__group" role="group">
              {parent ? null : (
                <span className="luna-floating-menu__group-label" aria-hidden="true">
                  {t(`floating.group.${group}`)}
                </span>
              )}
              {entries.map(({ item, index }) => (
                <div key={item.id} className="luna-floating-menu__item-row">
                  <button
                    id={`luna-ai-item-${item.id}`}
                    type="button"
                    role="option"
                    aria-selected={index === highlighted}
                    className="luna-floating-menu__item"
                    disabled={item.disabled}
                    onMouseEnter={() => {
                      setHighlighted(index);
                    }}
                    onClick={() => {
                      activate(item);
                    }}
                  >
                    <span className="luna-floating-menu__item-icon" aria-hidden="true">
                      {item.icon}
                    </span>
                    <span className="luna-floating-menu__item-label">{item.label}</span>
                    {item.children && item.children.length > 0 ? (
                      <ChevronRightIcon size={12} className="luna-floating-menu__item-more" />
                    ) : null}
                  </button>
                  {item.onRemove ? (
                    <IconButton
                      className="luna-floating-menu__item-remove"
                      label={t("floating.presets.delete")}
                      icon={<TrashIcon size={12} />}
                      onClick={item.onRemove}
                    />
                  ) : null}
                </div>
              ))}
            </div>
          ))}
        </div>
      ) : (
        <p className="luna-floating-menu__no-match">{t("floating.no_match")}</p>
      )}
      <p className="luna-floating-menu__hint">{hint}</p>
    </div>
  );
}
