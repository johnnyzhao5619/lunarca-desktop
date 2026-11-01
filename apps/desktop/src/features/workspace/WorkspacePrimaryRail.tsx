/**
 * 活动栏（rail，PRD-07-057 ①）：常驻纯图标条，只切换稳定对象空间。
 *
 * 内容空间在上、系统维护区贴底（PRD-07-002b）；可发现性由三态高亮 + hover 悬浮标签
 * （flyout）+ tooltip 补偿——它恒为图标条，不引入汉堡菜单或"展开成文字"模式。
 *
 * **顺序、显隐与锚点都不在这里判**：内容区画的是 Rust 归一化后的布局
 * （`useRailLayout`，PRD-07-003），加上「当前所在」与「录音进行中」两条前端才知道的
 * 临时显示（`rail-layout.ts`）。本组件只负责"画成什么样"。
 */

import { Fragment, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import {
  CaptionsIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  FilmIcon,
  FolderIcon,
  LayoutGridIcon,
  MessageIcon,
  MicIcon,
  SearchIcon,
  SettingsIcon,
  SparklesIcon,
  StickyNoteIcon,
  TrashIcon,
} from "../../components/ui/Icon";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu } from "../../lib/context-menu";
import type { RailLayoutItemDto } from "../../ipc/bindings";
import { usePointerDrag } from "../../lib/pointer-drag";
import { cssVariables } from "../../lib/css-variables";
import { useScrollEdges } from "../../lib/scroll-edges";
import { hiddenRailItems, isTemporarilyShown, moveRailItemToIndex, railScrollTarget, visibleRail } from "./rail-layout";
import {
  isAnchor,
  navBadgeToRender,
  railDividerIndices,
  spaceLabelKey,
  SYSTEM_SPACES,
  type NavBadge,
  type WorkspaceSpace,
} from "./workspace-ia";

/**
 * 空间 → 图标。
 *
 * **不放进 `WORKSPACE_IA`**：那是被无头测试直接 import 的纯数据模块，塞 JSX 会把
 * React 拖进去；而"加一个空间忘了给图标"由 `Record<WorkspaceSpace, …>` 的穷尽性当场
 * 编译拦下，不需要再造一层 icon-id 间接。
 */
const SPACE_ICONS: Record<WorkspaceSpace, ReactNode> = {
  search: <SearchIcon />,
  notes: <FolderIcon />,
  // 知识库 = 派生的知识矩阵（主题/标签/关系），用 AI 语义的星芒而非文件符号。
  knowledge: <SparklesIcon />,
  // 速记 = 便签/闪念纸张语义（PRD-07-066）：不用麦克风（文字侧同样落这里，且麦克风是
  // 「录音」的图标），不用收件托盘（邮件隐喻），不用与「笔记」相同的文件符号。
  quick_notes: <StickyNoteIcon />,
  // 录音 = 本机采集：麦克风同时是"这类内容是什么"与"怎么产生它"的直觉符号。
  recordings: <MicIcon />,
  // 转写 = 外来的音视频变成文字：字幕框。
  transcripts: <CaptionsIcon />,
  chat: <MessageIcon />,
  // 创作 = 做视频（ADR-162）：胶片，而不是笔或纸——那是「笔记」与「速记」的语义。
  creator: <FilmIcon />,
  // 应用 = 一组可启用的能力（ADR-165 ⑤）：四宫格，与「设置」的齿轮区分开。
  apps: <LayoutGridIcon />,
  trash: <TrashIcon />,
  settings: <SettingsIcon />,
};

export interface WorkspacePrimaryRailProps {
  activeSpace: WorkspaceSpace;
  /** Rust 归一化后的内容空间布局（含收起的，带位置）。 */
  layout: readonly RailLayoutItemDto[];
  /** 无论用户是否收起都要画出来的空间（录音进行中的「录音」，PRD-07-003）。 */
  forceVisible: ReadonlySet<WorkspaceSpace>;
  /** 角标按空间挂（PRD-07-066：只有「速记」有——未处理速记数）。 */
  badges: Partial<Record<WorkspaceSpace, NavBadge>>;
  /** 点击一级图标；再点当前项 = 收/开主侧栏由调用方判定（VS Code 心智）。 */
  onSelect: (space: WorkspaceSpace) => void;
  /** 右键「从侧栏隐藏 / 显示已隐藏的空间」——判定与落盘在 Rust，这里只派发。 */
  onSetVisible: (space: WorkspaceSpace, visible: boolean) => void;
  /** 拖拽排序落点（PRD-07-003 就地入口）：整条可见顺序一次交给 Rust compare-and-set。 */
  onReorder: (expected: readonly WorkspaceSpace[], ordered: readonly WorkspaceSpace[]) => void;
}

interface RailItem {
  space: WorkspaceSpace;
  locked: boolean;
  temporary: boolean;
}

/** 拖影相对指针的偏移（与文件树同一手感）。 */
const DRAG_GHOST_OFFSET = 12;

interface RailSpacesProps {
  items: readonly RailItem[];
  activeSpace: WorkspaceSpace;
  badges: Partial<Record<WorkspaceSpace, NavBadge>>;
  onSelect: (space: WorkspaceSpace) => void;
  /** 一格的右键菜单条目；`null` = 这一格没有右键动作（系统区、锚点）。 */
  onItemContextMenu: (event: ReactMouseEvent, item: RailItem) => void;
  /** 给了才可拖（内容区）；系统区（回收站 / 设置）没有顺序可言。 */
  onReorder?: (expected: readonly WorkspaceSpace[], ordered: readonly WorkspaceSpace[]) => void;
}

/**
 * 一段图标列表（内容空间 / 贴底系统区各一段）。
 *
 * 分隔线（PRD-07-003「区隔」）落在锚点连续段与非锚点连续段的边界上，按**画出来的**那几格
 * 重算：顺序可自定义、有的格会被收起，故区隔不能靠"锚点固定在顶部"。系统区全是锚点，
 * 天然没有分隔线；它与内容区之间的区隔一直由 `.luna-workspace__rail-footer` 的上边框在做。
 */
function RailSpaces({
  items,
  activeSpace,
  badges,
  onSelect,
  onItemContextMenu,
  onReorder,
}: RailSpacesProps) {
  const { t } = useTranslation("workspace");
  const [flyout, setFlyout] = useState<{ name: string; inline: number; block: number } | null>(null);
  function showFlyout(button: HTMLButtonElement, name: string) {
    const rect = button.getBoundingClientRect();
    setFlyout({ name, inline: rect.right, block: rect.top + rect.height / 2 });
  }
  useEffect(() => {
    const clear = () => setFlyout(null);
    window.addEventListener("scroll", clear, true);
    window.addEventListener("resize", clear);
    return () => {
      window.removeEventListener("scroll", clear, true);
      window.removeEventListener("resize", clear);
    };
  }, []);
  const dividers = railDividerIndices(items, (item) => item.locked);
  // 只有**用户让它显示的**格参与排序：临时显示的那格（收起了、只因在场才画出来）在
  // Rust 的可见顺序里不存在，把它算进 expected 会让每次 compare-and-set 都过期。
  const reorderable = items.filter((item) => !item.temporary).map((item) => item.space);
  const canDrag = (item: RailItem) => Boolean(onReorder) && !item.temporary;
  // 排序走**指针事件**（与文件树 / 标签条同一基元）：HTML5 DnD 与 Tauri 的 OS 文件拖放
  // 互斥——真机上 dragstart 有拖影、drop 却永远不来（2026-09-03）。落点 id = 「插到第 n 个
  // 之前」，尾部另给一个占满余量的落区表示"放到最后"。
  const drag = usePointerDrag<WorkspaceSpace>((space, dropId) => {
    const index = Number(dropId);
    if (!onReorder || !Number.isInteger(index)) return;
    const ordered = moveRailItemToIndex(reorderable, space, index);
    // 位置没变就不发请求（纯函数返回同一引用）。
    if (ordered !== reorderable) onReorder(reorderable, ordered);
  });
  const dropIndex = drag.payload === null || drag.dropId === null ? null : Number(drag.dropId);
  const dragGhost =
    drag.payload === null ? null : (
      <div
        className="luna-drag-ghost luna-workspace__rail-ghost"
        style={cssVariables({
          "--drag-x": `${drag.position.x + DRAG_GHOST_OFFSET}px`,
          "--drag-y": `${drag.position.y + DRAG_GHOST_OFFSET}px`,
        })}
      >
        {SPACE_ICONS[drag.payload]}
        <span>{t(spaceLabelKey(drag.payload))}</span>
      </div>
    );
  return (
    <>
      {items.map((item, index) => {
        const { space } = item;
        // 判据（>0 才渲染）是纯函数，与它的守护测试同源。
        const badge = navBadgeToRender(badges[space]);
        const name = t(spaceLabelKey(space));
        const label = badge ? t(badge.labelKey, { label: name, count: badge.count }) : name;
        // 这一格在可排序序列里的下标（落点 id 的语义：插到它之前）；临时显示的格不在序列里。
        const reorderIndex = canDrag(item) ? reorderable.indexOf(space) : null;
        return (
          <Fragment key={space}>
            {dividers.has(index) ? (
              <span
                className="luna-workspace__rail-divider"
                role="separator"
                aria-orientation="horizontal"
              />
            ) : null}
            <button
              type="button"
              className="luna-workspace__rail-btn"
              aria-current={space === activeSpace ? "page" : undefined}
              aria-label={label}
              title={label}
              onPointerEnter={(event) => showFlyout(event.currentTarget, name)}
              onPointerLeave={() => setFlyout(null)}
              onFocus={(event) => showFlyout(event.currentTarget, name)}
              onBlur={() => setFlyout(null)}
              // 临时显示的那格（用户收起了它，只因在场才画出来）给一个可见的差别：
              // 否则用户会以为"我明明收起了，怎么又回来了"。
              data-temporary={item.temporary || undefined}
              data-dragging={drag.payload === space || undefined}
              data-drop-before={reorderIndex !== null && dropIndex === reorderIndex ? true : undefined}
              data-drop-id={reorderIndex ?? undefined}
              onClick={() => { setFlyout(null); onSelect(space); }}
              onContextMenu={(event) => { setFlyout(null); onItemContextMenu(event, item); }}
              onPointerDown={(event) => {
                if (canDrag(item)) drag.begin(event, space);
              }}
            >
              {SPACE_ICONS[space]}
              {badge ? (
                <span
                  className="luna-workspace__rail-badge"
                  data-numeric={badge.numeric || undefined}
                  aria-hidden="true"
                >
                  {badge.numeric ? badge.count : null}
                </span>
              ) : null}
            </button>
          </Fragment>
        );
      })}
      {/* 尾部落区：占满活动栏余下的空白，表示"放到最后"；只在可排序的这一段渲染。 */}
      {onReorder ? (
        <span
          className="luna-workspace__rail-drop-end"
          data-drop-id={reorderable.length}
          data-drop-active={dropIndex === reorderable.length || undefined}
          aria-hidden="true"
        />
      ) : null}
      {dragGhost}
      {flyout ? createPortal(
        <span className="luna-workspace__rail-flyout" aria-hidden="true"
          style={cssVariables({ "--rail-label-inline": `${flyout.inline}px`, "--rail-label-block": `${flyout.block}px` })}>
          {flyout.name}
        </span>, document.body,
      ) : null}
    </>
  );
}

export function WorkspacePrimaryRail({
  activeSpace,
  layout,
  forceVisible,
  badges,
  onSelect,
  onSetVisible,
  onReorder,
}: WorkspacePrimaryRailProps) {
  const { t } = useTranslation("workspace");
  const menu = useContextMenu();

  const primary: RailItem[] = visibleRail(layout, activeSpace, forceVisible).map((item) => ({
    space: item.space,
    locked: item.locked,
    temporary: isTemporarilyShown(item, activeSpace, forceVisible),
  }));
  const system: RailItem[] = SYSTEM_SPACES.map((space) => ({
    space,
    locked: isAnchor(space),
    temporary: false,
  }));
  const hidden = hiddenRailItems(layout);

  /**
   * 短窗 / 大字号下内容区放不下全部空间：它仍是一条滚动区（PRD-07-067 ①：主导航恒可见，
   * 不收进「更多」），但滚动条是隐藏的——真机 2026-10-06（960×640、150%）「AI 对话」被挤到
   * 下面，看不见也没有任何「下面还有」的提示。还有内容的那一端出一枚滚动箭头（macOS 菜单
   * 放不下时的惯例：点一下翻一屏）并渐隐；当前空间（含从快捷键 / 别处跳进来的）滚进可见区。
   * 只渐隐不够：被遮的常是格与格之间的留白，图标本身没淡（真机 2026-10-06）。
   */
  const listRef = useRef<HTMLElement>(null);
  const primaryKey = primary.map((item) => item.space).join(",");
  const edges = useScrollEdges(listRef, [primaryKey]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>('[aria-current="page"]')?.scrollIntoView({ block: "nearest" });
  }, [activeSpace, primaryKey]);

  /**
   * 一格上的右键：非锚点给「从侧栏隐藏」；锚点与系统区**什么都不弹**但仍拦下原生菜单
   * （PRD-07-059 ③——空数组即宿主兜底）。
   */
  const openItemMenu = (event: ReactMouseEvent, item: RailItem) => {
    const name = t(spaceLabelKey(item.space));
    menu.open(
      event,
      buildMenu([
        !item.locked && {
          kind: "action",
          id: "rail_hide",
          label: t("rail.hide", { label: name }),
          onSelect: () => onSetVisible(item.space, false),
        },
      ]),
    );
  };

  /**
   * 活动栏空白处的右键：「显示已隐藏的空间 ▸」；一格都没收起时什么都不弹。
   *
   * 「空白」= 不落在任何一格按钮上（分隔线、按钮之间的间隙、列表下方都算）。判据不能是
   * `target === currentTarget`：真机上用户右键的是图标下面那一大片空白，而 `<nav>` 只到最后
   * 一格为止——那片空白根本不在它里面。故 `<nav>` 自身要撑满到系统区（CSS `flex: 1`），
   * 判据改成"不在按钮上"。
   */
  const openRailMenu = (event: ReactMouseEvent<HTMLElement>) => {
    if ((event.target as HTMLElement).closest("button")) {
      return;
    }
    menu.open(
      event,
      buildMenu([
        hidden.length > 0 && {
          kind: "submenu",
          id: "rail_show_hidden",
          label: t("rail.showHidden"),
          items: hidden.map((item) => ({
            kind: "action" as const,
            id: `rail_show_${item.space}`,
            label: t(spaceLabelKey(item.space)),
            onSelect: () => onSetVisible(item.space, true),
          })),
        },
      ]),
      { highlight: null },
    );
  };

  const scrollList = (direction: 1 | -1) => {
    const list = listRef.current;
    if (!list) return;
    list.scrollTo({ top: railScrollTarget(list.scrollTop, list.clientHeight, list.scrollHeight, direction), behavior: "smooth" });
  };

  return (
    <>
      {edges.start ? (
        <button
          type="button"
          className="luna-workspace__rail-scroll"
          aria-label={t("rail.scrollUp")}
          title={t("rail.scrollUp")}
          onClick={() => scrollList(-1)}
        >
          <ChevronUpIcon size={14} />
        </button>
      ) : null}
      <nav
        ref={listRef}
        className="luna-workspace__rail-list luna-scroll-fade"
        data-scroll-start={edges.start || undefined}
        data-scroll-end={edges.end || undefined}
        aria-label={t("nav.mainNav")}
        onContextMenu={openRailMenu}
      >
        <RailSpaces
          items={primary}
          activeSpace={activeSpace}
          badges={badges}
          onSelect={onSelect}
          onItemContextMenu={openItemMenu}
          onReorder={onReorder}
        />
      </nav>
      {edges.end ? (
        <button
          type="button"
          className="luna-workspace__rail-scroll"
          aria-label={t("rail.scrollDown")}
          title={t("rail.scrollDown")}
          onClick={() => scrollList(1)}
        >
          <ChevronDownIcon size={14} />
        </button>
      ) : null}
      <nav className="luna-workspace__rail-footer" aria-label={t("nav.systemNav")}>
        <RailSpaces
          items={system}
          activeSpace={activeSpace}
          badges={badges}
          onSelect={onSelect}
          onItemContextMenu={openItemMenu}
        />
      </nav>
    </>
  );
}
