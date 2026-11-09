import { useCallback, useEffect, useMemo, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  events,
  type NotificationCursorDto,
  type NotificationHistoryDto,
} from "../../ipc/bindings";
import { FilteredList } from "../command/FilteredList";
import { QueryBox } from "../../components/ui/QueryBox";
import { formatDateTime } from "../../lib/format-datetime";
import { EmptyState } from "../../components/ui/EmptyState";
import { useConfirm } from "../../components/ConfirmHost";
import { notificationSearchText } from "./notification-search";
import { bucketByTime } from "../../lib/time-buckets";
import { formatRelativeTime } from "../../lib/format-relative-time";
import { useNotificationLabels } from "./use-notification-labels";
import { NotificationDetail } from "./NotificationDetail";
import { NOTIFICATIONS_CHANGED_EVENT } from "../../lib/app-events";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator } from "../../lib/context-menu";
import {
  BellIcon,
  BookOpenIcon,
  CheckIcon,
  CloseIcon,
  CopyIcon,
  EllipsisIcon,
  ExternalWindowIcon,
  FilterIcon,
  TrashIcon,
} from "@lunarca/editor-core/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { goTo } from "../../lib/navigate";
import {
  NOTIFICATION_CLASSES,
  NO_NOTIFICATION_FILTER,
  filterNotifications,
  isNotificationFilterActive,
  toggleNotificationClass,
  type NotificationFilter,
} from "./notification-filter";

export interface NotificationHistoryProps {
  /**
   * 宿主可关闭时的关闭回调（通知抽屉传它）。给了就在**本面板自己的页头**里画关闭键。
   *
   * 不给宿主自己画：抽屉此前在标题行之上另起一条工具条只放一颗 ✕，于是同一个面板
   * 长了两个头。关闭是"对这个面板做的事"，它属于这个面板的动作区。
   */
  onClose?: () => void;
  /**
   * 详情模态开合的回调——**只给宿主一个事实，不给它控制权**。
   *
   * 唯一消费者是通知抽屉：两层模态的 Esc 监听都挂在 `document` 上，抽屉据此在详情开着
   * 时让出 Esc（否则按一次会连抽屉一起关掉）。旧「动态」页不是模态，不需要它。
   */
  onDetailOpenChange?: (open: boolean) => void;
}

/** 一页多少条。够大以致绝大多数设备一页装得下，又不至于一次把几千条塞进 DOM。 */
const PAGE_SIZE = 100;

export function NotificationHistory({ onClose, onDetailOpenChange }: NotificationHistoryProps = {}) {
  const { t, i18n } = useTranslation("notifications");
  const { t: tCommon } = useTranslation("common");
  const contextMenu = useContextMenu();
  const ask = useConfirm();
  const labelsOf = useNotificationLabels();
  const [items, setItems] = useState<NotificationHistoryDto[]>([]);
  /**
   * 表里一共有多少条 / 多少未读——**由后端给**，不数已加载的那一页。
   *
   * 页头此前直接渲染 `items.length`：一页 100 条，表里 500 条时它写着「100 条」，
   * 而那正是用户判断"要不要清"的依据。未读同理——只数这一页会与状态栏角标对不上
   * （PRD-15-062）。
   */
  const [totals, setTotals] = useState({ total: 0, unread: 0 });
  /** 下一页游标（`null` = 已经到底，不再显示「加载更多」）。 */
  const [nextCursor, setNextCursor] = useState<NotificationCursorDto | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<NotificationHistoryDto | null>(null);
  const [filter, setFilter] = useState<NotificationFilter>(NO_NOTIFICATION_FILTER);

  /**
   * 重取历史。`silent` 用于**新通知到达时的自动回流**——那一刻列表已经有内容，切一下
   * 「读取中」只会让面板闪一下；首次进入才需要交代它在读。
   */
  async function refresh(silent = false) {
    if (!silent) {
      setLoading(true);
    }
    const page = await commands.notificationHistory(PAGE_SIZE, null);
    setItems(page.items);
    setNextCursor(page.next_cursor);
    setTotals({ total: page.total, unread: page.unread });
    if (!silent) {
      setLoading(false);
    }
  }

  /**
   * 再取一页接在后面。
   *
   * **不重取整份**：重取会把用户已经翻到的位置一起丢掉，而且新通知随时在到达，
   * 整份重取的结果与他此刻看到的对不上。游标翻页只往下走。
   */
  async function loadMore() {
    if (!nextCursor || loadingMore) {
      return;
    }
    setLoadingMore(true);
    const page = await commands.notificationHistory(PAGE_SIZE, nextCursor);
    setItems((current) => [...current, ...page.items]);
    setNextCursor(page.next_cursor);
    setTotals({ total: page.total, unread: page.unread });
    setLoadingMore(false);
  }

  /**
   * 标已读；不给 `id` = 全部已读。
   *
   * 标单条**不整份重取**：用户可能已经「加载更多」翻了好几页，重取会把他翻到的位置
   * 一起丢掉。就地把这条改成已读，未读数按后端**实际改动了几条**扣减（它可能本来就已读，
   * 直接减 1 会把角标扣穿）。「全部已读」是另一回事——那一下状态全变了，整份重取才是对的。
   */
  async function markRead(id?: string) {
    const changed = await commands.notificationMarkRead(id ?? null);
    window.dispatchEvent(new CustomEvent(NOTIFICATIONS_CHANGED_EVENT));
    if (!id) {
      await refresh();
      return;
    }
    setItems((current) =>
      current.map((item) => (item.id === id ? { ...item, read: true } : item)),
    );
    setTotals((current) => ({ ...current, unread: Math.max(0, current.unread - changed) }));
  }

  async function clear() {
    // 清空不可撤销 → 先确认（破坏性操作，PRD-15-065）。确认走共享宿主的确认卡——
    // 此处此前是原生 `window.confirm`，而同一页另一个页签的「清空用量」压根不问。
    const ok = await ask({
      title: t("history.clear_title"),
      description: t("history.clear_desc"),
      confirmLabel: t("history.clear"),
    });
    if (!ok) {
      return;
    }
    await commands.clearNotificationHistory();
    window.dispatchEvent(new CustomEvent(NOTIFICATIONS_CHANGED_EVENT));
    await refresh();
  }

  /**
   * 只清已读（PRD-15-065 的非全量出口）。
   *
   * 它与「清空」是两件事，不能只留后者：为了不丢掉一条还没处理的未读，用户只能
   * 什么都不清——于是历史越积越长、越长越不敢清。已读＝看过并处理过的，清掉它不损失
   * 任何待办信息，故**不设二次确认**（确认是留给不可逆且会丢信息的动作的）。
   */
  async function clearRead() {
    await commands.clearReadNotificationHistory();
    window.dispatchEvent(new CustomEvent(NOTIFICATIONS_CHANGED_EVENT));
    await refresh();
  }

  // 点击一条通知：打开详情预览（读全文/元数据/深链动作），并顺手标记已读——
  // 让通知中心不再是「看完标题即止」的死胡同，深链跳转改由详情内的动作按钮触发。
  function activate(item: NotificationHistoryDto) {
    // 乐观置为已读：打开即读（邮件式），详情内状态立即反映，无需等 refresh 回填。
    setSelected({ ...item, read: true });
    onDetailOpenChange?.(true);
    if (!item.read) {
      void markRead(item.id);
    }
  }

  function closeDetail() {
    setSelected(null);
    onDetailOpenChange?.(false);
  }

  useEffect(() => {
    void refresh();
  }, []);

  /**
   * **抽屉开着的时候，新通知也得进来**。
   *
   * 此前这一页只在挂载时取一次，而状态栏角标订阅了 `notification://event` 实时 +1：
   * 抽屉开着时来一条新通知，**角标说有 3 条未读、列表里只有 2 条**——这正是 PRD-15-062
   * 「未读徽标与历史已读态一致」要根治的语义割裂，只不过它此前只被理解成"标已读之后
   * 角标要跟着降"的单向问题。
   *
   * 重取而不是把事件本身插进列表：`NotificationEventDto` 没有 `created_at_ms` 与 `read`
   * ——在前端现编这两格，等于让界面去伪造 Rust 拥有的事实（ADR-003）。
   */
  useEffect(() => {
    const unlisten = events.notificationEvent.listen(() => {
      void refresh(true);
    });
    return () => {
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, []);

  /**
   * 行右键（PRD-07-059）：查看详情 / 标已读 / 复制标题 / 前往（深链在场时）。
   *
   * **没有「删除这一条」**：通知历史是设备侧的事实记录，逐条删除会让"我明明看到过一条
   * 失败提示"变得无法追溯；要清理给的是整份清空（页头，破坏性动作带确认）。
   */
  function openItemMenu(event: ReactMouseEvent, item: NotificationHistoryDto) {
    const { title } = labelsOf(item);
    // 先取出来再判，闭包里 TS 收窄不到 `item.target`。
    const target = item.target;
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "detail",
          label: t("history.detail"),
          icon: <BookOpenIcon size={13} />,
          onSelect: () => activate(item),
        },
        target !== null && {
          kind: "action" as const,
          id: "navigate",
          label: t("history.go"),
          icon: <ExternalWindowIcon size={13} />,
          onSelect: () => {
            goTo(target);
          },
        },
        {
          kind: "action",
          id: "markRead",
          label: t("history.mark_read"),
          icon: <CheckIcon size={13} />,
          disabled: item.read,
          onSelect: () => void markRead(item.id),
        },
        {
          kind: "action",
          id: "copy",
          label: tCommon("menu.copy"),
          icon: <CopyIcon size={13} />,
          onSelect: () => {
            void commands.popupCopy(title);
          },
        },
        separator("danger"),
        {
          kind: "action",
          id: "clear",
          label: t("history.clear"),
          icon: <TrashIcon size={13} />,
          danger: true,
          disabled: items.length === 0,
          onSelect: () => void clear(),
        },
      ]),
    );
  }

  /* 三态各答各的：读取中 / 筛过但没命中（给一条清除筛选的出路）/ 本来就空
     （说清这里会记录什么，别把人晾在四个字前面）。 */
  const empty = useMemo(() => {
    if (loading) {
      return <p className="luna-empty">{t("history.loading")}</p>;
    }
    if (isNotificationFilterActive(filter) || query) {
      return (
        <EmptyState
          icon={<BellIcon size={24} />}
          title={t("history.empty_filtered")}
          action={
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => {
                setFilter(NO_NOTIFICATION_FILTER);
                setQuery("");
              }}
            >
              {t("history.clear_filter")}
            </button>
          }
        />
      );
    }
    return (
      <EmptyState
        icon={<BellIcon size={24} />}
        title={t("history.empty")}
        description={t("history.empty_hint")}
      />
    );
  }, [filter, loading, query, t]);
  const unreadCount = totals.unread;
  const hasUnread = unreadCount > 0;
  const hasRead = useMemo(() => items.some((item) => item.read), [items]);
  /** 先按条件筛，再交给列表做查询匹配——两层判据各归各处。 */
  const rows = useMemo(() => filterNotifications(items, filter), [filter, items]);
  const filtered = isNotificationFilterActive(filter);
  const shownCount = rows.length;

  /** 列表按时间倒序：查询在场时也不许按匹配得分重排（历史是时间流，见 `FilteredList.compare`）。 */
  const byTimeDesc = useCallback(
    (a: NotificationHistoryDto, b: NotificationHistoryDto) =>
      Number(b.created_at_ms) - Number(a.created_at_ms),
    [],
  );

  /** 时间分节：与速记流、录音/转写清单同一份分桶函数与同一套桶名（DRY）。 */
  const groups = useCallback(
    (list: NotificationHistoryDto[]) =>
      bucketByTime(list, (item) => Number(item.created_at_ms) || null, Date.now()).map(
        (bucket) => ({
          id: bucket.id,
          label: tCommon(`timeBucket.${bucket.id}`),
          items: bucket.items,
        }),
      ),
    [tCommon],
  );

  /** 类别筛选菜单：五档勾选 + 「全部类别」复位（与全应用同一套菜单语言，PRD-07-059）。 */
  function openFilterMenu(event: ReactMouseEvent) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "allClasses",
          label: t("history.filter_all_classes"),
          checked: filter.classes.length === 0,
          onSelect: () => setFilter((current) => ({ ...current, classes: [] })),
        },
        separator("classes"),
        ...NOTIFICATION_CLASSES.map((value) => ({
          kind: "action" as const,
          id: `class-${value}`,
          label: t(`history.class.${value}`),
          checked: filter.classes.includes(value),
          onSelect: () => setFilter((current) => toggleNotificationClass(current, value)),
        })),
      ]),
      { highlight: null },
    );
  }

  /** 收纳菜单：两个低频维护动作不占常显位（清除已读 / 清空历史）。 */
  function openMaintenanceMenu(event: ReactMouseEvent) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "clearRead",
          label: t("history.clear_read"),
          icon: <CheckIcon size={13} />,
          disabled: !hasRead,
          onSelect: () => void clearRead(),
        },
        separator("danger"),
        {
          kind: "action",
          id: "clear",
          label: t("history.clear"),
          icon: <TrashIcon size={13} />,
          danger: true,
          disabled: items.length === 0,
          onSelect: () => void clear(),
        },
      ]),
      { highlight: null },
    );
  }

  return (
    <section className="luna-notification-history">
      {/* 页头走全应用同一条页头原语（标题+徽标 | 动作，带下分隔线），与主区各页同一
          高度与基线；此前这一页自写了一份，于是抽屉一打开就"和别处不像"。 */}
      <header className="luna-main-header">
        <div className="luna-main-header__title">
          <h2>{t("history.title")}</h2>
          <span className="luna-count-badge">{totals.total}</span>
        </div>
        <div className="luna-main-header__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            disabled={!hasUnread}
            title={hasUnread ? undefined : t("history.no_unread")}
            onClick={() => void markRead()}
          >
            <CheckIcon size={14} />
            <span>{t("history.mark_all_read")}</span>
          </button>
          <IconButton
            label={t("history.maintenance_label")}
            icon={<EllipsisIcon size={14} />}
            aria-haspopup="menu"
            onClick={openMaintenanceMenu}
          />
          {onClose ? (
            <IconButton
              label={t("drawer.close")}
              icon={<CloseIcon size={14} />}
              onClick={onClose}
            />
          ) : null}
        </div>
      </header>

      {/* 视图条：**看什么**（范围 + 类别 + 关键词）与页头的**做什么**分开两层——
          此前四类控件平铺在一行里，用户读到的是一排没有关系的按钮。 */}
      <div className="luna-notification-history__viewbar">
        <div className="luna-notification-history__scope">
          {/* 「还有什么没处理」是这一页的头号问题，故未读是第一档而不是一个藏起来的筛选。 */}
          <div
            className="luna-segmented-control"
            role="radiogroup"
            aria-label={t("history.scope_label")}
          >
            {([false, true] as const).map((unreadOnly) => (
              <button
                key={String(unreadOnly)}
                type="button"
                role="radio"
                aria-checked={filter.unreadOnly === unreadOnly}
                className="luna-segmented-control__item"
                data-active={filter.unreadOnly === unreadOnly}
                onClick={() => setFilter((current) => ({ ...current, unreadOnly }))}
              >
                {unreadOnly
                  ? t("history.scope_unread", { count: unreadCount })
                  : t("history.scope_all")}
              </button>
            ))}
          </div>
          <IconButton
            label={t("history.filter_label")}
            icon={<FilterIcon size={14} />}
            aria-haspopup="menu"
            aria-pressed={filter.classes.length > 0}
            onClick={openFilterMenu}
          />
        </div>
        <QueryBox
          value={query}
          onChange={setQuery}
          placeholder={t("history.search_placeholder")}
          ariaLabel={t("history.search_label")}
          className="luna-notification-history__query"
        />
        {/* 筛过了就说清"这不是全部"，并给一条退出筛选的路；没筛时这一行不占位。 */}
        {filtered && (
          <p className="luna-notification-history__filter-note">
            <span>
              {t("history.count_filtered", { shown: shownCount, total: totals.total })}
            </span>
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => setFilter(NO_NOTIFICATION_FILTER)}
            >
              {t("history.clear_filter")}
            </button>
          </p>
        )}
      </div>
      <FilteredList
        className="luna-command-list luna-command-list--content luna-notification-history__list"
        items={rows}
        query={query}
        compare={byTimeDesc}
        groups={groups}
        getKey={(item) => item.id}
        getText={(item) => notificationSearchText(item, labelsOf(item))}
        empty={empty}
        onActivate={activate}
        rowContextMenu={openItemMenu}
        renderRow={({ item }) => {
          const { title, classLabel, severityLabel } = labelsOf(item);
          const absolute = formatDateTime(item.created_at_ms, i18n.language);
          return (
            /* 级别此前只是一行灰字，颜色只在**详情模态**里有——而"哪条是红的"恰恰是
               扫列表时唯一想知道的事。行首色条与详情共用同一组 `data-severity` 取值。 */
            <span className="luna-notification-history__row" data-severity={item.severity}>
              {/* 标题与时间同一行、时间靠右——三段各占一行的话，抽屉里十来条就把人埋了
                  （真机 2026-09-09）。这也是邮件与消息类产品的通行读法。 */}
              <span className="luna-notification-history__row-head">
                <strong
                  className={item.read ? undefined : "luna-notification-history__title--unread"}
                >
                  {title}
                </strong>
                {/* 相对时间 + `title` 里挂绝对时间：通知是流，「3 分钟前」比
                    「2026-09-09 14:32:07」有用得多；而要对时间线时那一格也没丢。 */}
                <small className="luna-notification-history__time" title={absolute}>
                  {formatRelativeTime(
                    Number.parseInt(item.created_at_ms, 10),
                    Date.now(),
                    i18n.language,
                  ) || absolute}
                </small>
              </span>
              {/* 已读态**必须有文字**：标题前的圆点与加粗是给"扫一眼"的，只靠它们
                  对读屏用户等于没有这条信息，对色觉障碍用户同理（`notification-drawer`
                  守护逐字锁定这一条）。 */}
              {/* 「未读」必须是**文字**：标题前的圆点与加粗是给"扫一眼"的，只靠它们对读屏
                  用户等于没有这条信息，对色觉障碍用户同理。已读则不再逐行复述一遍——它是
                  默认状态，每行写一次只是噪声，而"没有未读标记"本身就是读屏读得出的同一
                  件事（邮件类产品同此惯例）。 */}
              <small>
                {classLabel} · {severityLabel}
                {/* 分隔符是**版式**不是译文：拼进字符串会让标点与词序随语言写死一份
                    （`luna/no-translation-concatenation`）。 */}
                {item.read ? null : <> · {t("history.unread")}</>}
              </small>
            </span>
          );
        }}
        rowActions={(item) => (
          /* 标已读与前往是这一页最常做的两件事，此前**只藏在右键菜单里**——一等操作
             不该躲在二等入口后面。悬停/聚焦时显形，常态不抢标题的位置。 */
          <div className="luna-notification-history__row-actions">
            {item.target !== null ? (
              <IconButton
                density="sm"
                label={t("history.go")}
                icon={<ExternalWindowIcon size={13} />}
                onClick={() => {
                  goTo(item.target ?? "");
                }}
              />
            ) : null}
            <IconButton
              density="sm"
              label={t("history.mark_read")}
              icon={<CheckIcon size={13} />}
              disabled={item.read}
              onClick={() => void markRead(item.id)}
            />
          </div>
        )}
      />
      {/* 还有没加载的历史时**明说**：筛选与搜索都只作用在已加载的这几页上，不说清的话
          用户会把「没搜到」当成「历史里没有这条」。到底了就不显示。 */}
      {nextCursor && (
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-notification-history__more"
          disabled={loadingMore}
          onClick={() => void loadMore()}
        >
          {loadingMore
            ? t("history.loading")
            : t("history.load_more", { loaded: items.length, total: totals.total })}
        </button>
      )}

      {selected ? (
        <NotificationDetail
          item={selected}
          onClose={closeDetail}
          onNavigate={(target) => {
            // 详情里的业务深链：先收起详情，再派发——抽屉宿主据同一条事件关闭自己
            // 并把用户送到目标（PRD-15-064）。
            closeDetail();
            goTo(target);
          }}
        />
      ) : null}
    </section>
  );
}
