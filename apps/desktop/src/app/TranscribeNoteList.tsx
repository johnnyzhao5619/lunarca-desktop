import { type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { ListSortDto, SearchHitDto } from "../ipc/bindings";
import { FilmIcon, LinkIcon, SearchIcon } from "../components/ui/Icon";
import { IconButton } from "../components/ui/IconButton";
import { EmptyState } from "../components/ui/EmptyState";
import { FilteredNoteList } from "../features/workspace/FilteredNoteList";
import { BatchExportMenu } from "../features/transcribe/BatchExportMenu";
import { ArchiveListFilters } from "../features/transcribe/ArchiveListFilters";
import { formatListDuration, timeGroupingFor } from "../features/transcribe/transcribe-list";
import type { TranscribeImport } from "../features/transcribe/use-transcribe-import";
import type { NoteFolders } from "../features/workspace/use-note-folders";
import type { NoteSelection } from "../features/workspace/use-note-selection";
import { CaptureStartButton } from "../components/ui/CaptureStartButton";
import type { ListLoadState } from "../features/workspace/ListLoadMore";

export interface TranscribeNoteListProps {
  /** **已加载的那几页**（筛选与排序都已由 Rust 施加，此处不再重排，TDD-07 §4.1a）。 */
  items: SearchHitDto[];
  loadState: ListLoadState;
  /**
   * 筛选词与排序键**归工作区壳持有**：它们是取数口径的一部分（改了就要重新向 Rust 取），
   * 留在本组件里就只能筛已加载的页——那正是这次要修掉的错。
   */
  query: string;
  onQueryChange: (query: string) => void;
  sort: ListSortDto;
  onSortChange: (sort: ListSortDto) => void;
  /** 分页列尾（「加载更多」/ 重试）。 */
  footer?: ReactNode;
  selectedPath: string | null;
  onSelect: (path: string) => void;
  /** 多选（PRD-06-059）：与录音清单同一套原语，由工作区壳持有。 */
  selection: NoteSelection;
  /** 可见行上报（连选与方向键的取值域）。 */
  onVisibleRows?: (paths: string[]) => void;
  /** 行右键菜单（条目集由工作区壳给：打开方式 / 转写队列动作 / 导出 / 删除）。 */
  onItemContextMenu?: (event: ReactMouseEvent, item: SearchHitDto, paths: readonly string[]) => void;
  /** 行元数据（日期 / 处理状态胶囊）：与录音清单同一份渲染，由工作区壳给。 */
  itemMeta?: (item: SearchHitDto) => ReactNode;
  importer: TranscribeImport;
  onImport: () => void;
  /** 唤起「从链接导入」对话框（PRD-05-006）；「从链接导入」应用没启用时不给（ADR-167 ⑤）。 */
  onImportUrl?: () => void;
  onStatus: (message: string) => void;
  /** 文件夹能力（PRD-07-062）；由工作区壳注入，实现见 `use-note-folders.ts`。 */
  folders: NoteFolders;
}

/**
 * 「转写」主侧栏列表（PRD-05-030 / PRD-07-012）。
 *
 * 与「录音」列表同构（共用 `FilteredNoteList` 基座，DRY），差别在**主动作**（录音是
 * 「开始录音」= 现在采集，转写是「导入文件」= 已有文件要处理）与**列表控件**：
 * 转写是持续堆积的流水，故提供搜索 + 排序（最近修改 / 名称 / 时长，PRD-07-007）。
 *
 * **搜索与排序都只是在告诉 Rust 怎么取这份列表**（TDD-07 §4.1a）：本组件收到的
 * `items` 已经是筛过、排过、分好页的，它一个字都不再重算。此前这里挂着一份
 * `filterAndSortTranscripts`，筛的排的都只是已加载的那几页。
 *
 * 空态严格区分「从未导入」与「筛选无结果」（PRD-07-021）——后者给的是「清除筛选」，
 * 而不是让用户误以为库里没东西。批量导出作用于**已加载的当前结果**。
 *
 * **没有常驻的删除按钮**：删除走行右键、多选后的批量菜单与 Delete 键（PRD-07-003 /
 * PRD-06-059）。一颗红色描边的「删除」与主动作同尺寸并排，误触成本高，且与右键重复。
 */
export function TranscribeNoteList({
  items,
  loadState,
  query,
  onQueryChange,
  sort,
  onSortChange,
  footer,
  selectedPath,
  onSelect,
  selection,
  onVisibleRows,
  onItemContextMenu,
  itemMeta,
  importer,
  onImport,
  onImportUrl,
  folders,
  onStatus,
}: TranscribeNoteListProps) {
  const { t } = useTranslation("transcribe");

  const filtering = query.trim().length > 0;

  return (
    <FilteredNoteList
      noteType="transcript"
      items={items}
      loadState={loadState}
      footer={footer}
      selectedPath={selectedPath}
      onSelect={onSelect}
      selection={selection}
      onVisibleRows={onVisibleRows}
      onItemContextMenu={onItemContextMenu}
      itemBadge={(item) => formatListDuration(item.duration_ms)}
      itemMeta={itemMeta}
      groupTime={timeGroupingFor(sort)}
      rootDir={folders.rootDir}
      dirs={folders.dirs}
      onCreateFolder={folders.createFolder}
      onRenameFolder={folders.renameFolder}
      onDeleteFolder={folders.deleteFolder}
      onMoveInto={folders.moveInto}
      filtering={filtering}
      // 导出进页头计数行（与「新建文件夹」并排的图标键）：它作用于已加载的当前结果
      // （选中了几条就导出那几条；没有多选时仍是"当前这份结果"，与所见一致），不是
      // "开始一件事"的主动作。此前它与「从链接」并排占了侧栏第二行，转写侧栏因此比
      // 录音 / 速记多一行（真机 2026-09-08）。
      tools={
        items.length > 0 ? (
          <BatchExportMenu
            compact
            noteRels={selection.multiple ? selection.ordered : items.map((item) => item.path)}
            selected={selection.multiple}
            onStatus={onStatus}
          />
        ) : undefined
      }
      action={
        <div className="luna-capture-toolbar">
          {/* 「导入」是拆分按钮（PRD-05 §2）：主区选本地文件，尾部一枚「从链接」——两者是
              同一层动作的两个来源，尾键不藏进菜单（粘链接是最常用的路径之一，PRD-05-006）。 */}
          <div className="luna-transcribe-actions">
            {/* 拆分按钮的主半边：形态与另外三条侧栏同一枚。这一格没有全局快捷键——
                导入要先选文件，不存在"在别的应用里按一下就开始"这回事。 */}
            <CaptureStartButton
              icon={<FilmIcon size={14} />}
              label={importer.importing ? t("importing") : t("import")}
              disabled={importer.importing}
              onClick={onImport}
            />
            {onImportUrl ? (
              <IconButton
                className="luna-transcribe-actions__tail"
                label={t("fetch.entry")}
                icon={<LinkIcon size={14} />}
                onClick={onImportUrl}
              />
            ) : null}
          </div>
          {/* 正在筛选时**即使 0 条也要留着这排控件**：`items` 现在是筛过的结果，按它
              判显隐会让"筛出 0 条"连同搜索框一起消失，用户再也清不掉那个筛选词。 */}
          {(items.length > 0 || filtering) && (
            <ArchiveListFilters
              query={query}
              onQueryChange={onQueryChange}
              sort={sort}
              onSortChange={onSortChange}
              searchLabel={t("searchPlaceholder")}
            />
          )}
        </div>
      }
      emptyState={
        filtering ? (
          <EmptyState
            icon={<SearchIcon size={24} />}
            title={t("noMatchTitle")}
            description={t("noMatchBody")}
            action={
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                onClick={() => onQueryChange("")}
              >
                {t("clearFilter")}
              </button>
            }
          />
        ) : (
          <EmptyState
            icon={<FilmIcon size={24} />}
            title={t("emptyTitle")}
            description={t("emptyBody")}
          />
        )
      }
    />
  );
}
