// 笔记主区标签条（PRD-07-035/036，Obsidian/VS Code 对标）：
// 每个分屏 pane 一条：标签切换/关闭（含中键）/拖拽重排与跨 pane 移动/新建标签/
// 分屏/关闭分屏。纯呈现组件——布局变换全部回调到 `note-panes.ts` 状态机。

import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { useTablistNavigation } from "../../hooks/use-tablist-navigation";
import { useInlineOverflow } from "../../hooks/use-resize-observer";
import {
  ChevronDownIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CloseIcon,
  ExternalWindowIcon,
  FileIcon,
  PinFilledIcon,
  PinIcon,
  PlusIcon,
  SplitIcon,
} from "@lunarca/editor-core/Icon";
import { IconButton } from "../../components/ui/IconButton";
import {
  canGoBack,
  canGoForward,
  type NotePane,
  type TabAddress,
} from "./note-panes";
import { usePointerDrag } from "../../lib/pointer-drag";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator } from "../../lib/context-menu";
import type { MouseEvent as ReactMouseEvent } from "react";

function tabLabelOf(path: string | null, emptyLabel: string): string {
  if (!path) {
    return emptyLabel;
  }
  const name = path.split("/").pop() ?? path;
  return name.endsWith(".md") ? name.slice(0, -3) : name;
}

export interface NoteTabsBarProps {
  pane: NotePane;
  /** 该 pane 是否为活跃 pane（活跃 pane 的活跃标签有强调下划线）。 */
  paneActive: boolean;
  /** 活跃标签是否有未保存改动（脏点替代关闭按钮，hover 换回，VS Code 心智）。 */
  activeTabDirty: boolean;
  canSplit: boolean;
  canClosePane: boolean;
  /** 活跃标签是否指向一篇笔记（有路径才可弹出独立窗口）。 */
  canPopOut: boolean;
  onActivateTab: (tabId: number) => void;
  /** 双击标签 = 要留着它：预览标签转常驻（PRD-07-035）。 */
  onKeepTab?: (tabId: number) => void;
  onCloseTab: (tabId: number) => void;
  /** 固定 / 取消固定（PRD-07-035 W2b）。 */
  onTogglePin?: (tabId: number) => void;
  onCloseOtherTabs?: (tabId: number) => void;
  onCloseRightTabs?: (tabId: number) => void;
  onNewTab: () => void;
  /** 这一格的后退 / 前进（PRD-07-037，浏览器/IDE 心智；历史是 pane 级的）。 */
  onGoBack: () => void;
  onGoForward: () => void;
  onSplit: () => void;
  onClosePane: () => void;
  /** 把活跃标签移出到独立窗口（PRD-07-036 弹出窗口）。 */
  onPopOut: () => void;
  onMoveTab: (from: TabAddress, to: { paneId: number; index: number }) => void;
}

export function NoteTabsBar({
  pane,
  paneActive,
  activeTabDirty,
  canSplit,
  canClosePane,
  canPopOut,
  onActivateTab,
  onKeepTab,
  onCloseTab,
  onTogglePin,
  onGoBack,
  onGoForward,
  onCloseOtherTabs,
  onCloseRightTabs,
  onNewTab,
  onSplit,
  onClosePane,
  onPopOut,
  onMoveTab,
}: NoteTabsBarProps) {
  const { t } = useTranslation("workspace");
  const contextMenu = useContextMenu();
  // 标签区横向溢出时给「列出所有标签页」菜单（VS Code 标签栏尾部的 ⌄ 同心智）：被滚出
  // 视野的标签否则只能靠拖滚动条找。溢出与否既随盒子宽度变、也随标签数变，故内容键传标签数。
  const tabsRef = useRef<HTMLDivElement | null>(null);
  const tabsOverflow = useInlineOverflow(tabsRef, pane.tabs.length);

  // 标签重排走**指针事件**（与文件树同一基元）：HTML5 DnD 与 Tauri 的 OS 文件拖放
  // 互斥，改用指针事件后二者兼得（见 `lib/pointer-drag.ts`）。
  const drag = usePointerDrag<TabAddress>((from, dropId) => {
    const index = Number(dropId);
    if (!Number.isInteger(index)) {
      return;
    }
    onMoveTab(from, { paneId: pane.id, index });
  });
  const dropIndex =
    drag.payload === null || drag.dropId === null ? null : Number(drag.dropId);
  // 标签的**顺序**就是键盘 ←/→ 的顺序；值取 id 的字符串形式（hook 用它做 ref 索引）。
  const tabs = useTablistNavigation(
    pane.tabs.map((tab) => String(tab.id)),
    String(pane.activeTabId),
    (value) => onActivateTab(Number(value)),
  );

  return (
    <div
      className="luna-tabbar"
      role="tablist"
      data-pane-active={paneActive || undefined}
      data-drop-id={pane.tabs.length}
    >
      {/* 后退/前进排在标签**之前**——浏览器与 IDE 都在这个位置，而位置本身就是它的说明。
          禁用态的判据只有一份（`canGoBack` / `canGoForward`），界面不自己数下标。 */}
      <div className="luna-tabbar__nav">
        <IconButton
          label={t("tabsbar.goBack")}
          icon={<ChevronLeftIcon size={13} />}
          disabled={!canGoBack(pane)}
          onClick={onGoBack}
        />
        <IconButton
          label={t("tabsbar.goForward")}
          icon={<ChevronRightIcon size={13} />}
          disabled={!canGoForward(pane)}
          onClick={onGoForward}
        />
      </div>
      <div className="luna-tabbar__tabs" ref={tabsRef}>
        {pane.tabs.map((tab, index) => {
          const active = tab.id === pane.activeTabId;
          const dirty = active && activeTabDirty;
          const pinned = Boolean(tab.pinned);
          return (
            // 外层此前是 `<div role="tab">`：读屏念得出「标签 2/4」，Tab 键却根本进不去。
            // 声明了角色不兑现交互，比不声明更糟。改成 tablist > presentation > tab 的标准
            // 结构——外层只承担拖拽命中与视觉外壳（`role="presentation"`，不进无障碍树），
            // 真正的 tab 是里面那颗按钮；关闭钮与它并列，而不是嵌在按钮里（按钮里套按钮
            // 是无效 HTML，读屏会把两者念成同一个控件）。
            <div
              key={tab.id}
              role="presentation"
              className="luna-tabbar__tab"
              data-active={active || undefined}
              // 预览标签（PRD-07-035）：标题斜体，下一次浏览会复用它。
              data-preview={tab.preview || undefined}
              data-pinned={pinned || undefined}
              data-drop-before={dropIndex === index || undefined}
              data-drop-id={index}
              onPointerDown={(event) =>
                drag.begin(event, { paneId: pane.id, tabId: tab.id })
              }
              onContextMenu={(event: ReactMouseEvent) => {
                // 不在这里 preventDefault：宿主见到已被拦下的事件就让位（最内层先认领的判据），
                // 自己先拦等于把自己的菜单也拦掉——标签右键此前因此一次都没弹出来过（真机
                // 2026-10-05）。拦原生菜单是 `contextMenu.open` 的事。
                event.stopPropagation();
                onActivateTab(tab.id);
                contextMenu.open(
                  event,
                  buildMenu([
                    ...(onTogglePin && tab.path
                      ? [
                          {
                            kind: "action" as const,
                            id: "togglePin",
                            label: pinned ? t("tabsbar.unpinTab") : t("tabsbar.pinTab"),
                            icon: <PinIcon size={13} />,
                            onSelect: () => onTogglePin(tab.id),
                          },
                          separator("pin"),
                        ]
                      : []),
                    {
                      kind: "action",
                      id: "closeTab",
                      label: t("tabsbar.closeTab"),
                      icon: <CloseIcon size={13} />,
                      onSelect: () => onCloseTab(tab.id),
                    },
                    ...(onCloseOtherTabs && pane.tabs.length > 1
                      ? [
                          {
                            kind: "action" as const,
                            id: "closeOthers",
                            label: t("tabsbar.closeOthers"),
                            onSelect: () => onCloseOtherTabs(tab.id),
                          },
                        ]
                      : []),
                    ...(onCloseRightTabs && index < pane.tabs.length - 1
                      ? [
                          {
                            kind: "action" as const,
                            id: "closeRight",
                            label: t("tabsbar.closeRight"),
                            onSelect: () => onCloseRightTabs(tab.id),
                          },
                        ]
                      : []),
                    ...(tab.path && canPopOut
                      ? [
                          separator("popOut"),
                          {
                            kind: "action" as const,
                            id: "popOut",
                            label: t("tabsbar.popOut"),
                            icon: <ExternalWindowIcon size={13} />,
                            onSelect: onPopOut,
                          },
                        ]
                      : []),
                  ]),
                );
              }}
              onAuxClick={(event) => {
                // 中键关闭（浏览器/IDE 惯例）；固定标签不吃中键（VS Code 默认同此），要关走右键菜单。
                if (event.button === 1 && !pinned) {
                  event.preventDefault();
                  onCloseTab(tab.id);
                }
              }}
            >
              <button
                ref={tabs.register(String(tab.id))}
                type="button"
                role="tab"
                aria-selected={active}
                tabIndex={tabs.tabIndexOf(String(tab.id))}
                className="luna-tabbar__tab-trigger"
                title={tab.path ?? t("tabsbar.emptyTab")}
                onClick={() => onActivateTab(tab.id)}
                onDoubleClick={() => onKeepTab?.(tab.id)}
                onKeyDown={tabs.onKeyDown}
              >
                <FileIcon size={12} className="luna-tabbar__tab-icon" />
                <span className="luna-tabbar__tab-label">
                  {tabLabelOf(tab.path, t("tabsbar.emptyTab"))}
                </span>
              </button>
              {/* 固定标签的这一格是图钉而不是关闭钮（VS Code 心智）：点它取消固定，
                  免得顺手一点把特意留着的标签关掉。脏点规则两者相同。 */}
              <button
                type="button"
                className="luna-tabbar__tab-close"
                data-dirty={dirty || undefined}
                data-pinned={pinned || undefined}
                aria-label={pinned ? t("tabsbar.unpinTab") : t("tabsbar.closeTab")}
                title={pinned ? t("tabsbar.unpinTab") : t("tabsbar.closeTab")}
                onClick={(event) => {
                  event.stopPropagation();
                  if (pinned) {
                    onTogglePin?.(tab.id);
                  } else {
                    onCloseTab(tab.id);
                  }
                }}
              >
                {dirty ? (
                  <span className="luna-tabbar__dirty-dot" aria-hidden="true" />
                ) : null}
                {pinned ? (
                  <PinFilledIcon size={11} className="luna-tabbar__close-icon" />
                ) : (
                  <CloseIcon size={11} className="luna-tabbar__close-icon" />
                )}
              </button>
            </div>
          );
        })}
        <IconButton
          label={t("tabsbar.newTab")}
          icon={<PlusIcon size={13} />}
          onClick={onNewTab}
        />
      </div>
      <div className="luna-tabbar__pane-actions">
        {tabsOverflow ? (
          <IconButton
            label={t("tabsbar.listTabs")}
            icon={<ChevronDownIcon size={13} />}
            aria-haspopup="menu"
            onClick={(event) => {
              contextMenu.open(
                event,
                buildMenu(
                  pane.tabs.map((tab) => ({
                    kind: "action" as const,
                    id: `tab-${tab.id}`,
                    label: tabLabelOf(tab.path, t("tabsbar.emptyTab")),
                    title: tab.path ?? undefined,
                    checked: tab.id === pane.activeTabId,
                    onSelect: () => onActivateTab(tab.id),
                  })),
                ),
                { label: t("tabsbar.listTabs") },
              );
            }}
          />
        ) : null}
        {canPopOut ? (
          <IconButton
            label={t("tabsbar.popOut")}
            icon={<ExternalWindowIcon size={13} />}
            onClick={onPopOut}
          />
        ) : null}
        {canSplit ? (
          <IconButton
            label={t("tabsbar.splitRight")}
            icon={<SplitIcon size={13} />}
            onClick={onSplit}
          />
        ) : null}
        {canClosePane ? (
          <IconButton
            label={t("tabsbar.closePane")}
            icon={<CloseIcon size={13} />}
            onClick={onClosePane}
          />
        ) : null}
      </div>
    </div>
  );
}
