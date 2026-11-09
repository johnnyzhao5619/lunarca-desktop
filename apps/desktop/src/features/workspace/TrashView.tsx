// 回收站视图（系统组，PRD-06-009 / PRD-07）：已删除笔记的可还原/永久删除列表。
// 与通知历史同一套「一屏一列表」页面语言（页头标题+副标题+工具栏，FilteredList 搜索基座
// DRY）：破坏性「清空回收站」收敛为页头右侧描边小按钮（不再是整宽红条），行操作「还原/
// 永久删除」定宽不换行，文件名醒目、原目录与删除时间次级弱化。命令自持（restore/purge），
// 与 `NotificationHistory` 自洽同构；结果经回调回流父级刷新（DRY，前端零业务）。
//
// **这一页有四态，不是一个清单**（PRD-06-009 明文：系统废纸篓策略下跳系统 Trash、永久
// 删除策略下无回收站视图）。此前它无条件渲染库内 `.trash/` 的清单，而 `vault.delete_mode`
// **默认是系统废纸篓**——开箱即用的用户删掉一篇笔记，点进「回收站」看到的是「回收站为空 ·
// 删除笔记后会先落到这里」，而那句话在默认策略下是假的：安全网在心理层面直接失效。形态
// 判定是纯函数 [trashViewState]，策略由 Rust 归一化后下发（ADR-003）。

import { useEffect, useMemo, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { commands, type TrashEntryDto } from "../../ipc/bindings";
import { FilteredList } from "../command/FilteredList";
import {
  ArrowClockwiseIcon,
  BookOpenIcon,
  CopyIcon,
  ExternalWindowIcon,
  FolderIcon,
  SortIcon,
  TrashIcon,
} from "@lunarca/editor-core/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator } from "../../lib/context-menu";
import { formatDateTime } from "../../lib/format-datetime";
import { vaultErrorCode, vaultErrorText } from "../../lib/vault-error";
import { FileNodeIcon } from "./NoteTree";
import { TrashPreview } from "./TrashPreview";

import { EmptyState } from "../../components/ui/EmptyState";
import { QueryBox } from "../../components/ui/QueryBox";
import { useConfirm, type ConfirmOptions } from "../../components/ConfirmHost";
import { trashViewState, type DeleteModeState } from "../../lib/delete-mode";
import {
  EMPTY_SELECTION,
  orderedSelection,
  prunedSelection,
  selectionAfterClick,
  type SelectionState,
} from "./note-selection";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { trashComparator, TRASH_SORT_MODES, type TrashSortMode } from "./trash-sort";
import { summarizeRestoreBatch } from "./trash-batch";
import { goTo } from "../../lib/navigate";

interface TrashViewProps {
  entries: TrashEntryDto[];
  /** 当前生效的删除策略（`null` = 尚未取回；此刻这一页什么都不说，见 [trashViewState]）。 */
  mode: DeleteModeState;
  /** 还原成功 → 打开还原后的笔记路径。 */
  onRestored: (path: string) => void;
  /** 还原/删除后刷新库树与列表。 */
  onChanged: () => void;
  /**
   * 回流状态栏的一句话。
   *
   * 它此前叫 `onError`，而「已还原为副本：xxx」是一条**成功**消息——同一个出口既送
   * 错误也送结果时，名字只能诚实地说它是什么（状态栏），不能只说它最常见的那一种用法。
   */
  onStatus: (message: string) => void;
}

/**
 * 剩余几天算「临期」——到这一档就该变色催一下。
 *
 * 3 天是常见口径（Notion / Drive 到期前几天开始提示）；再短用户就来不及反应，再长则
 * 天天都在报警、警示色也就不再有意义。
 */
const EXPIRY_SOON_DAYS = 3;

/** 从库相对路径拆出「文件名」与「所在目录」两段（目录为空 = 库根）。 */
function splitPath(path: string): { name: string; folder: string } {
  const slash = path.lastIndexOf("/");
  return slash < 0
    ? { name: path, folder: "" }
    : { name: path.slice(slash + 1), folder: path.slice(0, slash) };
}

export function TrashView({ entries, mode, onRestored, onChanged, onStatus }: TrashViewProps) {
  const { t, i18n } = useTranslation("workspace");
  const { t: tCommon } = useTranslation("common");
  const contextMenu = useContextMenu();
  const ask = useConfirm();
  const [query, setQuery] = useState("");
  const [sortMode, setSortMode] = useState<TrashSortMode>("deleted");
  /** 过滤后还剩几条（由列表回报，见 `FilteredList.onVisibleCountChange`）。 */
  const [visibleCount, setVisibleCount] = useState(entries.length);
  /**
   * 勾选集（批量还原 / 批量永久删除）。键是**条目 id 不是原路径**：同一个路径可以被删
   * 两次（删掉、又新建同名、再删），路径在回收站里不唯一。
   *
   * 选择代数复用 `note-selection.ts` 这份纯函数（Cmd 加选、Shift 连选、收敛到在场行）——
   * 文件树与五条清单用的是同一份，多选的语义不该在这一页另说一遍。
   */
  const [selection, setSelection] = useState<SelectionState>(EMPTY_SELECTION);
  // 预览中的条目（点击行唤起模态；还原/删除成功后清空关闭）。
  const [previewing, setPreviewing] = useState<TrashEntryDto | null>(null);

  /**
   * 还原一条；落点被占时**给出路而不是给措辞**。
   *
   * 此前失败一律回落成 `status.error`——状态栏上只剩「错误」两个字，而后端明明发的是
   * `VAULT_ALREADY_EXISTS`（`common.json` 里早就有一句合格文案，同一个文件里的
   * `TrashPreview` 也一直在用 `vaultErrorText`）。更要紧的是：原位置被占时，用户手里
   * 那篇笔记**根本取不回来**，除非他先自己去把占位的那个挪开。访达「放回原处」遇冲突
   * 给的是「两者都保留」，Notion / Drive 干脆自动改名——可恢复性不该被一次重名卡死。
   */
  async function restore(id: string) {
    const result = await commands.restore(id, null);
    if (result.status === "ok") {
      setPreviewing(null);
      onRestored(result.data);
      onChanged();
      return;
    }
    if (vaultErrorCode(result.error) !== "VAULT_ALREADY_EXISTS") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    const entry = entries.find((item) => item.id === id);
    const keepBoth = await ask({
      title: t("main.restoreConflictTitle", { path: entry?.original_path ?? "" }),
      description: t("main.restoreConflictDesc"),
      confirmLabel: t("main.restoreAsCopy"),
      // 留副本不删不改任何东西，它是这张卡上**更安全**的那一边。
      destructive: false,
    });
    if (!keepBoth) {
      return;
    }
    const copied = await commands.restore(id, "keep_both");
    if (copied.status === "ok") {
      setPreviewing(null);
      onStatus(t("main.restoredAsCopy", { path: copied.data }));
      onRestored(copied.data);
      onChanged();
    } else {
      onStatus(vaultErrorText(copied.error));
    }
  }

  // 确认走共享宿主（`.luna-dialog` 确认卡）——此处此前是原生 `window.confirm`。
  async function purge(id: string | null, confirmOptions: ConfirmOptions) {
    if (!(await ask(confirmOptions))) {
      return;
    }
    const result = await commands.purge(id);
    if (result.status === "error") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    setPreviewing(null);
    onChanged();
    // **删不掉的那几件要说出来**：后端此前把 `remove_file` 的失败整个吞了，界面照报
    // 成功——文件夹条目因此变成既看不见、又删不掉的幽灵。现在它们留在回收站里可重试，
    // 而这一句是用户知道"没删干净"的唯一途径。
    if (result.data.failed.length > 0) {
      onStatus(t("main.purgeFailedSome", { count: result.data.failed.length }));
    }
  }

  /* 空态两态各答各的：有查询词 = 没匹配上（给「清空查询」这条出路），没查询词 = 回收站
     本来就是空的（说清它是干什么的）。说明文字此前是**内联在 TSX 里的中文裸串**，英文
     界面下直接显示中文。 */
  const empty = query ? (
    <EmptyState
      icon={<TrashIcon size={24} />}
      title={t("main.emptyList")}
      action={
        <button type="button" className="luna-btn luna-btn--secondary" onClick={() => setQuery("")}>
          {tCommon("action.clearQuery")}
        </button>
      }
    />
  ) : (
    <EmptyState
      icon={<TrashIcon size={24} />}
      title={t("main.emptyTrash")}
      description={t("main.emptyTrashHint")}
    />
  );

  /** 行右键：预览 / 还原 / 复制原路径 / 永久删除（与行内按钮同一批动作，DRY）。 */
  function openEntryMenu(event: ReactMouseEvent, entry: TrashEntryDto) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "preview",
          label: t("main.trashPreview"),
          icon: <BookOpenIcon size={13} />,
          onSelect: () => setPreviewing(entry),
        },
        // 原路径未知的条目不给「还原」（同行内动作，理由见那里）。
        entry.original_path !== "" && {
          kind: "action" as const,
          id: "restore",
          label: t("main.restoreFromTrash"),
          icon: <ArrowClockwiseIcon size={13} />,
          onSelect: () => void restore(entry.id),
        },
        entry.original_path !== "" && {
          kind: "action" as const,
          id: "copyPath",
          label: tCommon("menu.copyPath"),
          icon: <CopyIcon size={13} />,
          onSelect: () => {
            void commands.popupCopy(entry.original_path).then((result) => {
              if (result.status === "error") {
                onStatus(t("status.error"));
              }
            });
          },
        },
        separator("danger"),
        {
          kind: "action",
          id: "purge",
          label: t("main.purgeOne"),
          icon: <TrashIcon size={13} />,
          danger: true,
          onSelect: () =>
            void purge(entry.id, {
              title: t("main.purgeOneTitle", { path: entry.original_path }),
              description: t("main.irreversible"),
              confirmLabel: t("main.purgeOne"),
            }),
        },
      ]),
    );
  }

  /** 页面空白处右键：作用于整个回收站的动作。 */
  function openViewMenu(event: ReactMouseEvent) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "purgeAll",
          label: t("main.purgeAll"),
          icon: <TrashIcon size={13} />,
          danger: true,
          disabled: entries.length === 0,
          onSelect: () =>
            void purge(null, {
              title: t("main.purgeAllTitle"),
              description: t("main.purgeAllDesc"),
              confirmLabel: t("main.purgeAll"),
            }),
        },
      ]),
      { highlight: null },
    );
  }

  /**
   * 这一页此刻的形态（PRD-06-009）。`mode` 未解析时是 `pending`：三张说明卡各自承诺
   * 一件相反的事，先摆一张再换掉等于让用户读到一句当时为假的话。
   */
  const viewState = trashViewState(mode, entries.length);
  const showList = viewState === "list" || viewState === "listLegacy";
  /** 库内回收站此刻一共占多少——用户决定要不要清空时，唯一真正相关的数字。 */
  const occupiedBytes = entries.reduce((sum, item) => sum + Number(item.byte_size), 0);
  const comparator = useMemo(() => trashComparator(sortMode), [sortMode]);

  /** 此刻按屏幕顺序排好的 id（连选取区间、收敛勾选集都按它）。 */
  const orderedIds = useMemo(
    () => [...entries].sort(comparator).map((entry) => entry.id),
    [comparator, entries],
  );
  /** 条目变了（还原 / 永久删除 / 自动裁剪之后）就把勾选集收敛到还在场的那些。 */
  useEffect(() => {
    setSelection((current) => prunedSelection(current, orderedIds));
  }, [orderedIds]);

  const picked = useMemo(() => orderedSelection(selection, orderedIds), [orderedIds, selection]);
  const clearSelection = () => setSelection(EMPTY_SELECTION);

  /** 勾选框点击：Shift 连选、Cmd/Ctrl 加选，其余为切换这一条（复用同一份选择代数）。 */
  function togglePick(id: string, event: ReactMouseEvent) {
    setSelection((current) =>
      selectionAfterClick(
        current,
        id,
        { toggle: !event.shiftKey, range: event.shiftKey },
        orderedIds,
      ),
    );
  }

  /**
   * 批量还原。
   *
   * **逐条报结果，不中止整批**：一批里通常只有一两条撞名，因为那一条而让另外八条也
   * 还不回来是最糟的处理。撞名的收拢成一句，并给一次「全部还原为副本」的出路——与单条
   * 那张确认卡同一条产品判断，只是问一次而不是问八次。
   */
  async function restoreSelection() {
    const targets = picked;
    const conflicts: string[] = [];
    let restored = 0;
    let failed = 0;
    /** 头一条真失败的原因——一条都没回来时它是用户唯一的线索。 */
    let reason: string | undefined;
    for (const id of targets) {
      const result = await commands.restore(id, null);
      if (result.status === "ok") {
        restored += 1;
      } else if (vaultErrorCode(result.error) === "VAULT_ALREADY_EXISTS") {
        conflicts.push(id);
      } else {
        failed += 1;
        reason ??= vaultErrorText(result.error);
      }
    }
    let conflicted = conflicts.length;
    if (conflicts.length > 0) {
      const keepBoth = await ask({
        title: t("main.restoreConflictBatchTitle", { count: conflicts.length }),
        description: t("main.restoreConflictDesc"),
        confirmLabel: t("main.restoreAsCopy"),
        destructive: false,
      });
      if (keepBoth) {
        for (const id of conflicts) {
          const copied = await commands.restore(id, "keep_both");
          if (copied.status === "ok") {
            restored += 1;
            conflicted -= 1;
          } else {
            conflicted -= 1;
            failed += 1;
            reason ??= vaultErrorText(copied.error);
          }
        }
      }
    }
    clearSelection();
    onChanged();
    /* **一次批量动作只写一句状态栏**。此前三种去向各写各的，而状态栏只有一行——
       后写的把先写的盖掉，最后那句「已还原 N 项」尤其致命：它会把"另外两条没回来"整个
       抹掉，用户看到的是一句纯好消息（真机 2026-09-09）。口径归 `trash-batch.ts`。 */
    const summary = summarizeRestoreBatch({ restored, conflicted, failed }, reason);
    if (summary) {
      onStatus(t(summary.key, summary.params));
    }
  }

  /** 批量永久删除：**一次确认**管整批（逐条问八遍不是更安全，是更容易被无脑点过去）。 */
  async function purgeSelection() {
    const targets = picked;
    if (
      !(await ask({
        title: t("main.purgeSelectedTitle", { count: targets.length }),
        description: t("main.irreversible"),
        confirmLabel: t("main.purgeOne"),
      }))
    ) {
      return;
    }
    let failed = 0;
    for (const id of targets) {
      const result = await commands.purge(id);
      if (result.status === "error") {
        onStatus(vaultErrorText(result.error));
        failed += 1;
      } else {
        failed += result.data.failed.length;
      }
    }
    clearSelection();
    setPreviewing(null);
    onChanged();
    if (failed > 0) {
      onStatus(t("main.purgeFailedSome", { count: failed }));
    }
  }

  /** 排序菜单：三档口径，勾选态即当前口径（与全应用同一套菜单语言，PRD-07-059）。 */
  function openSortMenu(event: ReactMouseEvent) {
    contextMenu.open(
      event,
      buildMenu(
        TRASH_SORT_MODES.map((option) => ({
          kind: "action" as const,
          id: `sort-${option}`,
          label: t(`main.trashSort.${option}`),
          checked: sortMode === option,
          onSelect: () => setSortMode(option),
        })),
      ),
      { highlight: null },
    );
  }

  /** 去改删除方式（设置 › 存储与隐私 › 笔记库，深链定位到那一项）。 */
  function openDeleteModeSetting() {
    goTo("settings://notes/vault.delete_mode");
  }

  /** 打开系统废纸篓（`trash` 策略下这一页唯一的实质动作）。 */
  async function openSystemTrash() {
    const result = await commands.openSystemTrash();
    if (result.status === "error") {
      onStatus(t("main.trashOpenSystemFailed", { error: result.error }));
    }
  }

  return (
    // 右键的整页动作（清空回收站）只在真有清单时才有意义；说明卡上右键落回宿主兜底。
    <section className="luna-trash" onContextMenu={showList ? openViewMenu : undefined}>
      {/* 页头走全应用同一条主区页头原语（标题+徽标 | 动作，带下分隔线）——此前这一页
          自写了一份"标题上、副标题下、工具栏浮在右上"的布局，正是 PRD-07-071 说的
          「每页都不太一样」里的那一半。 */}
      <header className="luna-main-header">
        <div className="luna-main-header__title">
          <h1>{t("main.trashTitle")}</h1>
          {showList && (
            <>
              <span className="luna-count-badge">
                {/* 有查询词时说清「筛出几条 / 一共几条」——否则徽标写着 12、列表里只有 2 行。 */}
                {query
                  ? t("main.trashCountFiltered", { shown: visibleCount, total: entries.length })
                  : entries.length}
              </span>
              {/* 页头只留**此刻真正相关的那个数字**（占了多少）。此前这里还有一整句
                  「已删除的笔记暂存于此…」——有条目在场时它是模板话，而"这一页是干什么的"
                  已由空态、删除确认卡与另两态的说明卡各自答过一遍（PRD-07-065 同一事实
                  同屏只说一次）。 */}
              {occupiedBytes > 0 && (
                <span className="luna-trash__occupied">
                  {t("main.trashOccupied", { size: formatBytes(occupiedBytes, i18n.language) })}
                </span>
              )}
            </>
          )}
        </div>
        {showList && (
          <div className="luna-main-header__actions">
            <button
              type="button"
              className="luna-btn luna-btn--danger-subtle luna-btn--sm luna-trash__purge-all"
              disabled={entries.length === 0}
              title={t("main.purgeAll")}
              onClick={() =>
                void purge(null, {
                  title: t("main.purgeAllTitle"),
                  description: t("main.purgeAllDesc"),
                  confirmLabel: t("main.purgeAll"),
                })
              }
            >
              <TrashIcon size={14} />
              <span>{t("main.purgeAll")}</span>
            </button>
          </div>
        )}
      </header>

      {/* 勾选之后的批量条：**它说清动作作用在谁身上**，并给一条退出多选的路。放在清单
          正上方而不是页头里——它是随勾选出现/消失的临时物，混进页头会让那一行时高时矮。 */}
      {picked.length > 0 && (
        <div className="luna-trash__batch">
          <span className="luna-trash__batch-count">
            {t("main.selectedCount", { count: picked.length })}
          </span>
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm luna-trash__action"
            onClick={() => void restoreSelection()}
          >
            <ArrowClockwiseIcon size={14} />
            <span>{t("main.restoreFromTrash")}</span>
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--danger-subtle luna-btn--sm luna-trash__action"
            onClick={() => void purgeSelection()}
          >
            <TrashIcon size={14} />
            <span>{t("main.purgeOne")}</span>
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={clearSelection}
          >
            {t("main.clearSelection")}
          </button>
        </div>
      )}

      {/* 视图条：**看什么**（关键词 + 排序）与页头的**做什么**分两层——与通知历史对称。
          搜索框不能待在 `__actions` 里：那一区是 `flex-wrap: wrap`，而标题是可增长项，
          于是动作区被压到"只有搜索框那么宽"、排序与清空被挤到第二行（1400px 窗口依然
          如此，静态复现于 `__preview`）。 */}
      {showList && (
        <div className="luna-trash__viewbar">
          <QueryBox
            value={query}
            onChange={setQuery}
            placeholder={t("main.trashSearchPlaceholder")}
            ariaLabel={t("main.trashSearchLabel")}
            className="luna-trash__query"
          />
          <IconButton
            label={t("main.trashSortLabel")}
            icon={<SortIcon size={14} />}
            aria-haspopup="menu"
            onClick={openSortMenu}
          />
        </div>
      )}

      {/* 策略已切走、库内却还留着条目：照列不误，另说一句为什么它们还在。
          少了这一支就会把用户还能还原的数据连同这一页一起藏起来。 */}
      {viewState === "listLegacy" && mode !== null && (
        <p className="luna-trash__notice">
          {t("main.trashLegacyNotice", {
            mode: t(`settings:option.vault.delete_mode.${mode}`),
            count: entries.length,
          })}
        </p>
      )}

      {viewState === "system" && (
        <EmptyState
          icon={<ExternalWindowIcon size={24} />}
          title={t("main.trashSystemTitle")}
          description={t("main.trashSystemDesc")}
          action={
            <div className="luna-trash__state-actions">
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                onClick={() => void openSystemTrash()}
              >
                <ExternalWindowIcon size={14} />
                <span>{t("main.trashOpenSystem")}</span>
              </button>
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={openDeleteModeSetting}
              >
                {t("main.trashSwitchToVault")}
              </button>
            </div>
          }
        />
      )}

      {viewState === "permanent" && (
        <EmptyState
          icon={<TrashIcon size={24} />}
          title={t("main.trashPermanentTitle")}
          description={t("main.trashPermanentDesc")}
          action={
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              onClick={openDeleteModeSetting}
            >
              {t("main.trashChangeDeleteMode")}
            </button>
          }
        />
      )}

      {showList && (
      <FilteredList<TrashEntryDto>
        className="luna-command-list luna-command-list--content luna-trash__list"
        items={entries}
        query={query}
        getKey={(entry) => entry.id}
        getText={(entry) => entry.original_path}
        empty={empty}
        compare={comparator}
        onVisibleCountChange={setVisibleCount}
        onActivate={(entry) => setPreviewing(entry)}
        rowContextMenu={openEntryMenu}
        rowLeading={(item) => (
          /* 勾选框在行首、与「打开预览」正交：
             点开与勾选是两个意图，故它是那颗按钮的兄弟而不是它的内容。 */
          <input
            type="checkbox"
            className="luna-trash__pick"
            checked={selection.paths.has(item.id)}
            aria-label={t("main.selectEntry", { path: item.original_path })}
            onChange={() => undefined}
            onClick={(event) => togglePick(item.id, event)}
          />
        )}
        renderRow={({ item }) => {
          /* 原路径为空 = 索引损坏后从磁盘重建出来的条目（见 `Vault::load_trash_index`）：
             它的原路径只存在于那份坏掉的索引里，重建不出来。如实说"不知道它原来在哪儿"，
             而不是编一个。 */
          const recovered = item.original_path === "";
          const { name, folder } = splitPath(item.original_path);
          /* 次级行四段，缺哪段就不占位：原目录（库根为空）· 占用 · 删除时间 · 还有几天。
             「还有几天」只在保留期不是「永不」时有值（天数由 Rust 算好，ADR-003）。 */
          const meta = [
            folder,
            formatBytes(Number(item.byte_size), i18n.language),
            t("main.deletedAt", { time: formatDateTime(item.deleted_ms, i18n.language) }),
          ].filter((part) => part !== "");
          const daysLeft = item.expires_in_days;
          return (
            <div className="luna-trash__row">
              {/* 文件夹此前落到 `FileNodeIcon` 的兜底分支被画成一个文件——回收站里
                  "这是一个目录、删掉它等于删掉里面全部内容"恰恰是最该一眼看出的事。 */}
              {item.is_dir ? (
                <FolderIcon size={14} className="luna-trash__row-icon" />
              ) : (
                <FileNodeIcon path={item.original_path} className="luna-trash__row-icon" />
              )}
              <div className="luna-trash__row-main">
                <strong>{recovered ? t("main.trashRecoveredName") : name}</strong>
                <small>
                  {recovered ? t("main.trashRecoveredHint") : null}
                  {recovered && meta.length > 0 ? " · " : null}
                  {meta.join(" · ")}
                  {/* 剩余天数**单独一格并在临期时变色**：它是这一行里唯一会让人现在
                      就动手的信息（对标 Notion / Drive / 备忘录的「N 天后删除」）。混在
                      前面那串灰字里等于没说。 */}
                  {daysLeft !== null && (
                    <span
                      className="luna-trash__expiry"
                      data-soon={daysLeft <= EXPIRY_SOON_DAYS}
                    >
                      {t("main.trashExpiresIn", { count: daysLeft })}
                    </span>
                  )}
                </small>
              </div>
            </div>
          );
        }}
        rowActions={(item) => (
          /* **两颗动作不等重**。「还原」是这一页存在的理由，常显；「永久删除」不可逆，
             降为悬停/键盘聚焦才显形的危险图标钮——把它和还原并排画成两颗同样大的键，等于
             邀请误点。它在右键菜单与预览卡里仍然一等可达。 */
          <div className="luna-trash__row-actions">
            {/* 原路径未知就**不给「还原」**：还不知道目的地就往库里写，比让用户自己去
                系统文件管理器里取回糟得多。预览与永久删除仍然可用。 */}
            {item.original_path !== "" && (
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm luna-trash__action"
                onClick={() => void restore(item.id)}
              >
                <ArrowClockwiseIcon size={14} />
                <span>{t("main.restoreFromTrash")}</span>
              </button>
            )}
            <IconButton
              density="sm"
              tone="danger"
              className="luna-trash__purge-one"
              label={t("main.purgeOne")}
              icon={<TrashIcon size={14} />}
              onClick={() =>
                void purge(item.id, {
                  title: t("main.purgeOneTitle", { path: item.original_path }),
                  description: t("main.irreversible"),
                  confirmLabel: t("main.purgeOne"),
                })
              }
            />
          </div>
        )}
      />
      )}

      {previewing && (
        <TrashPreview
          entry={previewing}
          onClose={() => setPreviewing(null)}
          onRestore={() => void restore(previewing.id)}
          onPurge={() =>
            void purge(previewing.id, {
              title: t("main.purgeOneTitle", { path: previewing.original_path }),
              description: t("main.irreversible"),
              confirmLabel: t("main.purgeOne"),
            })
          }
        />
      )}
    </section>
  );
}
