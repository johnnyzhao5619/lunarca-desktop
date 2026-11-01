import {
  Fragment,
  useEffect,
  useMemo,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { fuzzyMatch, type FuzzyMatch } from "./fuzzy";
import { paginate } from "../../lib/paginate";
import { Pager } from "../../components/ui/Pager";
import { isImeKeyEvent } from "../../lib/ime-key-event";

export interface FilteredListRow<T> {
  item: T;
  match: FuzzyMatch;
}

export interface FilteredListProps<T> {
  items: T[];
  query: string;
  getKey: (item: T) => string;
  getText: (item: T) => string;
  /** 自定义匹配（返回 null = 过滤掉）。缺省为对 getText 的模糊匹配。 */
  getMatch?: (item: T, query: string) => FuzzyMatch | null;
  /**
   * 结果排序口径。缺省按**模糊匹配得分**降序——命令面板要的正是相关度。
   *
   * **时间流必须传自己的比较器**：回收站与通知历史是按时间倒序的流，用户来这里找的
   * 几乎总是"刚刚那一条"；缺省口径下只要一输入查询，得分重排就会把它甩到列表中间，
   * 而且同分项的先后还随输入内容变。传了比较器就完全按调用方的口径排，得分不参与。
   */
  compare?: (a: T, b: T) => number;
  renderRow: (row: FilteredListRow<T>) => ReactNode;
  /**
   * 选中一行。`how.split` = 用户按着 ⌘ / Ctrl（回车或点击）：调用方据此在分屏打开
   * （快速切换器，PRD-07-040 ③）；不区分的调用方忽略第二个参数即可。
   */
  onActivate: (item: T, how: { split: boolean }) => void;
  /**
   * `⇧Enter`：对**查询本身**做事（快速切换器按查询词新建笔记，PRD-07-040 ③）。没有结果
   * 行时也会触发——那正是它的主场。不传则 `⇧Enter` 与回车相同。
   */
  onSubmitQuery?: () => void;
  onCancel?: () => void;
  rowActions?: (item: T) => ReactNode;
  /**
   * 行**首**插槽（多选勾选框）。与 `rowActions` 对称：都是行主体那颗按钮的**兄弟**，
   * 不是它的内容。
   *
   * 按钮不能嵌按钮：嵌了的话键盘走一遍只能走到外层那颗，读屏念出来的也是外层的名字。
   * 而「勾上这几条」与「打开这一条」本来就是两个意图。
   */
  rowLeading?: (item: T) => ReactNode;
  /** 行右键菜单（条目集由调用方给；缺省则落到宿主兜底，PRD-07-059）。 */
  rowContextMenu?: (event: ReactMouseEvent, item: T) => void;
  /**
   * 时间/主题分节。给了就在清单里插入分节标题；**不改顺序**——它只是按 `compare` 排好的
   * 那一串切成几段（与速记流 / 录音清单同一条 `bucketByTime` 口径，DRY）。
   *
   * 分节标题是 `role="none"` 的 `li`：`listbox` 的可选项必须是 `option`，把标题也做成
   * 一个可选项会让键盘上下键停在一行不能选的字上（与 `FilteredNoteList` 同一形态）。
   */
  groups?: (items: T[]) => Array<{ id: string; label: string; items: T[] }>;
  /**
   * 过滤后还剩几条——**页头的计数徽标要的是这个数**。
   *
   * 由列表回报而不是让调用方再算一遍：查询的匹配口径（模糊匹配、自定义 `getMatch`）
   * 只在这里，外面复算一份就等于开了第二个事实源，而两者一旦分叉，用户会看到"徽标说
   * 3 条、列表里 5 行"。
   */
  onVisibleCountChange?: (count: number) => void;
  /**
   * 每页条数；不传 = 不分页（既有行为）。
   *
   * **切页发生在筛选与排序之后**：先分后筛的话，第 2 页会是"未筛选集合的第 2 页"，
   * 而用户以为自己在看搜索结果的第 2 页。键盘导航同样只在**当前页**内走——方向键能
   * 停在一行看不见的条目上，比不能翻页更糟。
   *
   * **与 `groups` 互斥**：分节是把排好的一串切成几段，分页是把它切成几页，两者叠在
   * 一起时"第 2 页的第 1 节"要么半截要么重复，而这两个功能没有一个调用方同时需要。
   */
  pageSize?: number;
  /** 分页控件的读屏名（同屏可能有两份列表各自分页）。`pageSize` 在场时必给。 */
  pageLabel?: string;
  className?: string;
  empty?: ReactNode;
  /**
   * 搜索框（在列表之外、由调用方持有）：方向键 / Enter / Esc 从它转发到列表。
   * 没有这条转发，用户打完字按 Enter 什么也不发生——键盘处理只挂在列表容器上，而焦点
   * 还在输入框里；此前得先 Tab 到列表再按 Enter（真机 2026-09-02 命令面板执行「没反应」）。
   */
  inputRef?: RefObject<HTMLInputElement | null>;
}

export function FilteredList<T>({
  items,
  query,
  getKey,
  getText,
  getMatch,
  compare,
  renderRow,
  onActivate,
  onSubmitQuery,
  onCancel,
  rowActions,
  rowLeading,
  rowContextMenu,
  groups,
  onVisibleCountChange,
  pageSize,
  pageLabel,
  className,
  empty,
  inputRef,
}: FilteredListProps<T>) {
  const rows = useMemo(
    () =>
      items
        .map((item) => {
          const match = getMatch ? getMatch(item, query) : fuzzyMatch(query, getText(item));
          return match ? { item, match } : null;
        })
        .filter((row): row is FilteredListRow<T> => row !== null)
        .sort((a, b) => (compare ? compare(a.item, b.item) : b.match.score - a.match.score)),
    [compare, getMatch, getText, items, query],
  );
  /**
   * 渲染序列：分节在场时是「标题 + 该节的行」交替，否则就是行本身。
   *
   * **当前项的下标只数行、不数标题**：把标题也算进去的话，方向键会在标题上"停一下"，
   * 而 `aria-activedescendant` 指向一个不存在的选项 id。
   */
  const sections = useMemo(() => {
    if (!groups) {
      return null;
    }
    const byItem = new Map(rows.map((row) => [row.item, row] as const));
    let index = 0;
    return groups(rows.map((row) => row.item)).map((group) => ({
      id: group.id,
      label: group.label,
      rows: group.items.flatMap((item) => {
        const row = byItem.get(item);
        return row ? [{ row, index: index++ }] : [];
      }),
    }));
  }, [groups, rows]);

  const [activeIndex, setActiveIndex] = useState(0);
  const [page, setPage] = useState(0);

  // 筛选口径变了就回到第一页：留在第 5 页而结果只剩 2 页时，用户看到的是"搜到了东西
  // 却一片空白"（`paginate` 会夹住页码，但那一下的跳转没有道理让用户自己发现）。
  useEffect(() => {
    setPage(0);
  }, [query]);

  /** 当前页（不分页时就是全部）。键盘与渲染都只认它。 */
  const visible = useMemo(
    () => (pageSize ? paginate(rows, page, pageSize) : { items: rows, page: 0, pageCount: 1 }),
    [page, pageSize, rows],
  );

  useEffect(() => {
    setActiveIndex(0);
  }, [query, visible.items.length, visible.page]);

  useEffect(() => {
    onVisibleCountChange?.(rows.length);
  }, [onVisibleCountChange, rows.length]);

  function activate(index: number, split = false) {
    const row = visible.items[index];
    if (row) {
      onActivate(row.item, { split });
    }
  }

  /** 列表容器与搜索框共用同一份按键处理（列表的 `onKeyDown` 也走它）。 */
  function handleKey(
    event: Pick<KeyboardEvent, "key" | "preventDefault" | "shiftKey" | "metaKey" | "ctrlKey">,
  ): void {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((current) => Math.min(current + 1, visible.items.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((current) => Math.max(current - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (event.shiftKey && onSubmitQuery) {
        onSubmitQuery();
      } else {
        activate(activeIndex, event.metaKey || event.ctrlKey);
      }
    } else if (event.key === "Escape") {
      event.preventDefault();
      onCancel?.();
    }
  }

  useEffect(() => {
    const input = inputRef?.current;
    if (!input) {
      return;
    }
    // 输入法组字与上屏那一下的 Enter / Esc / 方向键属于输入法，不是在选行、关面板。
    // 只判 `isComposing` 在 WKWebView 里拦不住上屏那一下（PRD-07-040 ①）。
    const onKeyDown = (event: KeyboardEvent) => {
      if (isImeKeyEvent(event)) {
        return;
      }
      handleKey(event);
    };
    input.addEventListener("keydown", onKeyDown);
    return () => {
      input.removeEventListener("keydown", onKeyDown);
    };
  });

  function renderItem(row: FilteredListRow<T>, index: number) {
    const key = getKey(row.item);
    return (
      <li key={key} id={`${key}-option`} role="option" aria-selected={index === activeIndex}>
        {rowLeading ? (
          <div className="luna-command-list__leading">{rowLeading(row.item)}</div>
        ) : null}
        <button
          type="button"
          data-active={index === activeIndex}
          onMouseEnter={() => {
            setActiveIndex(index);
          }}
          onClick={(event) => {
            activate(index, event.metaKey || event.ctrlKey);
          }}
          onContextMenu={(event) => {
            // 右键先把该行置为当前项，菜单动作与高亮行才是同一条。
            setActiveIndex(index);
            rowContextMenu?.(event, row.item);
          }}
        >
          <span>{renderRow(row)}</span>
        </button>
        {rowActions ? (
          <div className="luna-command-list__actions">{rowActions(row.item)}</div>
        ) : null}
      </li>
    );
  }

  return (
    <div
      className={className}
      role="listbox"
      tabIndex={0}
      aria-activedescendant={
        visible.items[activeIndex]
          ? `${getKey(visible.items[activeIndex].item)}-option`
          : undefined
      }
      onKeyDown={handleKey}
    >
      {rows.length === 0 ? (
        empty
      ) : (
        <ul className="luna-command-list__items">
          {sections
            ? sections.map((section) => (
                <Fragment key={`group-${section.id}`}>
                  <li role="none" className="luna-command-list__group">
                    <h3 className="luna-command-list__group-title">{section.label}</h3>
                  </li>
                  {section.rows.map(({ row, index }) => renderItem(row, index))}
                </Fragment>
              ))
            : visible.items.map((row, index) => renderItem(row, index))}
        </ul>
      )}
      {pageSize ? (
        <Pager
          page={visible.page}
          pageCount={visible.pageCount}
          onPageChange={setPage}
          label={pageLabel ?? ""}
        />
      ) : null}
    </div>
  );
}
