/**
 * 速记流（PRD-06-058 A/A′/C、PRD-07-066）——「速记」空间侧栏：捕获胶囊 + 查找筛选 + 时间桶。
 *
 * 收录口径由类型驱动（`type: quick-note`，判定在 Rust），收的是文字与语音速记的产物；被
 * 「标记为待整理」的普通笔记留在「笔记」空间。速记转正即换类型，自然离开这条流。
 *
 * 三处刻意的形态选择：
 * - 时间桶而不是文件夹树：这里是查询投影，条目散落在整个 Vault，按目录分组会问出一个它
 *   答不上来的问题；文件夹归属仍归文件树。
 * - 相对时龄常驻，让「该整理了」的压力可见；超过陈化门槛的行带轻警示色（`stale` 由 Rust
 *   用与筛选同一个常量判定，PRD-06-055）。
 * - 三条出路各占一枚行内动作、不折进菜单：转为正式笔记 / 生成知识笔记 / 在编辑器中打开
 *   既不互斥也不互相取代。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "../../components/ui/EmptyState";
import { IconButton } from "../../components/ui/IconButton";
import {
  BookOpenIcon,
  FolderPlusIcon,
  MicIcon,
  PencilIcon,
  SparklesIcon,
  StickyNoteIcon,
} from "../../components/ui/Icon";
import type { NoteSelection } from "../workspace/use-note-selection";
import { formatClock } from "../../lib/format-duration";
import { formatRelativeTime } from "../../lib/format-relative-time";
import type { DraftFilterDto, DraftNoteDto } from "../../ipc/bindings";
import { FolderedItems } from "../workspace/FolderedItems";
import { listHasNothing } from "../workspace/note-folder-tree";
import type { ListLoadState } from "../workspace/ListLoadMore";
import type { NoteFolders } from "../workspace/use-note-folders";
import { QueryBox } from "../../components/ui/QueryBox";
import { CaptureBox } from "./CaptureBox";
import { bucketDrafts } from "./draft-inbox";

/**
 * 筛选档的呈现顺序（`DraftFilterDto` 的穷尽投影，少一格 TS 当场编译不过）。
 *
 * 顺序按用户找东西的频率排，不按枚举定义序：先全部，再两个最常用的事实，最后是催办那档。
 */
const DRAFT_FILTERS: readonly DraftFilterDto[] = ["all", "with_audio", "refined", "stale"];

export interface DraftInboxProps {
  /** 已加载的那几页（TDD-07 §4.1a）；时间桶按已到手的条目分组，随加载逐步长出。 */
  notes: readonly DraftNoteDto[];
  loadState: ListLoadState;
  /**
   * 当前筛选下的条数（Rust 下发的权威计数，页头徽标用）。
   *
   * 不是 `notes.length`（那只是已加载的几页），也不是全库总数——后者归活动栏角标
   * （PRD-07-066），一筛选就跟着变的角标是在说谎。
   */
  total: number;
  /** 当前筛选档（判定在 Rust，这里只是"用户点了哪一档"）。 */
  filter: DraftFilterDto;
  onFilterChange: (filter: DraftFilterDto) => void;
  /** 就地查找的关键词（未去抖的原值——输入框要跟手）。 */
  query: string;
  onQueryChange: (query: string) => void;
  /** 文件夹能力（PRD-07-062）：与录音 / 转写 / 对话同一个 hook、同一批 Vault 命令。 */
  folders: NoteFolders;
  /** 分页列尾（「加载更多」/ 重试）。 */
  footer?: React.ReactNode;
  selectedPath: string | null;
  onSelect: (path: string) => void;
  /** 在编辑器中打开：在「笔记」目的地的工作面另开一个标签。 */
  onOpenInNotes: (path: string) => void;
  /**
   * 转为正式笔记（`settle_note_as_knowledge`）。
   *
   * 撤销条不在这条侧栏里：转正之后主区空了出来，而撤销要出现在用户视线所在处——从主区
   * 那颗按钮转正的人眼睛不在侧栏。状态与呈现一并上移到宿主，两个入口共用同一条。
   */
  onSettle: (path: string) => void;
  /** 生成知识笔记（PRD-06-054）；源速记留在原地不动。 */
  onDistill: (path: string) => void;
  onCapture: (text: string, options: { open: boolean }) => Promise<boolean>;
  onVoiceCapture: () => void;
  /** 「展开文字捕获并聚焦」的请求序号（见 `CaptureBox.openRequest`）。 */
  captureOpenRequest: number;
  /** 语音输入全局快捷键（已格式化，空串 = 未设置）——原样交给语音按钮。 */
  voiceShortcut: string;
  /**
   * 文字速记全局快捷键（已格式化，空串 = 未设置）。
   *
   * 只出现在空态脚注里，不挂在捕获框上：框此刻就在屏幕上，说「按这个键能打开一个输入框」
   * 是废话；空态要讲的是「在别的应用里也能记」（PRD-06-058 B″）。
   */
  textShortcut: string;
  /**
   * 多选（PRD-06-059）：与笔记树、录音、转写、标签结果共用同一套选择原语。
   *
   * 速记此前自带一套行首勾选框加一条「已选 N 条」工具条——同一件事在应用里于是有两种
   * 做法与两种外观。多选统一为 Cmd/Ctrl 加选、Shift 连选，批量动作走右键菜单。
   */
  selection: NoteSelection;
  /** 此刻可见的行按屏幕顺序上报（选择集与锚点恒在它上面收敛）。 */
  onVisibleRows: (paths: readonly string[]) => void;
  /** 行右键（复用唯一那份笔记菜单，PRD-07-059）；`targets` = 动作真正作用的那一批。 */
  onItemContextMenu: (
    event: React.MouseEvent,
    note: DraftNoteDto,
    targets: readonly string[],
  ) => void;
}

export function DraftInbox({
  notes,
  loadState,
  total,
  filter,
  onFilterChange,
  query,
  onQueryChange,
  folders,
  footer,
  selectedPath,
  onSelect,
  onOpenInNotes,
  onSettle,
  onDistill,
  onCapture,
  onVoiceCapture,
  captureOpenRequest,
  voiceShortcut,
  textShortcut,
  selection,
  onVisibleRows,
  onItemContextMenu,
}: DraftInboxProps) {
  const { t, i18n } = useTranslation("workspace");
  // 相对时龄按"这一次渲染的此刻"算：`Date.now()` 不进纯函数（同 `format-relative-time`）。
  const now = Date.now();
  /** 新建文件夹的行内输入态（与另外四条清单同一形态，由 `FolderedItems` 渲染）。 */
  const [creatingFolder, setCreatingFolder] = useState(false);
  /** 此刻有筛选在场——空态、脚注与「清除筛选」都看它。 */
  const filtering = filter !== "all" || query.trim() !== "";
  /** 空态脚注：两条全局键的真实绑定（未绑定就不提）。 */
  const shortcutFootnote =
    [
      textShortcut && t("draft.emptyShortcutText", { shortcut: textShortcut }),
      voiceShortcut && t("draft.emptyShortcutVoice", { shortcut: voiceShortcut }),
    ]
      .filter(Boolean)
      .join(" · ") || undefined;

  return (
    <section className="luna-inbox" aria-busy={loadState.loading}>
      {/* 页头与另外四条清单同形（PRD-07-071，2026-09-08 真机上报「上面仍没有统计数字和
          创建文件夹」）。此前这里刻意没有页头，理由是"条数在活动栏图标上已经有一份"——
          但七个空间里只有这一个没有，用户读到的是"这一页跟别的页不一样"，而不是"这里
          省了一份重复"。同一事实出现两次的代价，小于同一套界面有两种说法。 */}
      <header className="luna-sidebar-header">
        <div className="luna-sidebar-header__title">
          {(notes.length > 0 || (loadState.exhausted && !loadState.loading && !loadState.failed)) && (
            <span className="luna-count-badge">{total}</span>
          )}
          {folders.rootDir !== undefined && (
            <IconButton
              label={t("tree.newFolder")}
              icon={<FolderPlusIcon size={14} />}
              onClick={() => setCreatingFolder(true)}
            />
          )}
        </div>
      </header>
      <CaptureBox
        onCapture={onCapture}
        onVoiceCapture={onVoiceCapture}
        voiceShortcut={voiceShortcut}
        openRequest={captureOpenRequest}
      />

      {/* 就地查找 + 四档筛选（PRD-06-058 A′）。
          判定全在 Rust：前端就地筛「已加载的那几页」会当场说谎——清单一分页，被筛掉的
          条目里有多少还没取回来，界面根本不知道。这里交出去的只有用户敲了什么、点了哪
          一档（ADR-003）。只在有东西可筛时才出现——全库一条都没有时，筛选器比空态碍事。 */}
      {(total > 0 || filter !== "all" || query !== "") && (
        <div className="luna-inbox__filters">
          <QueryBox
            value={query}
            onChange={onQueryChange}
            placeholder={t("draft.filterPlaceholder")}
            ariaLabel={t("draft.filterLabel")}
          />
          <div className="luna-inbox__chips" role="group" aria-label={t("draft.filterLabel")}>
            {DRAFT_FILTERS.map((id) => (
              <button
                key={id}
                type="button"
                className="luna-inbox__chip"
                // 单选一组：`aria-pressed` 说的是"这一档开着"，与勾选集那种多选不是一回事。
                aria-pressed={filter === id}
                onClick={() => onFilterChange(id)}
              >
                {t(`draft.filter.${id}`)}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* 空态只在**真的**没东西时出现：有目录或正在输入新建行就渲染树（`listHasNothing`）。 */}
      {loadState.exhausted && !loadState.loading && !loadState.failed && listHasNothing({
        itemCount: notes.length,
        dirCount: folders.dirs.length,
        creating: creatingFolder,
      }) ? (
        <EmptyState
          icon={<StickyNoteIcon size={24} />}
          // 筛不到不等于一条都没有（PRD-07-020）：拿首次空态去答「没有匹配」，用户会以为
          // 自己的速记全没了。有筛选在场时换一套话术，并给一条出路。
          title={filtering ? t("draft.noMatchTitle") : t("draft.emptyTitle")}
          description={filtering ? t("draft.noMatchBody") : t("draft.emptyBody")}
          action={
            filtering ? (
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                onClick={() => {
                  onQueryChange("");
                  onFilterChange("all");
                }}
              >
                {t("draft.clearFilters")}
              </button>
            ) : undefined
          }
          // 两条全局键都取自设置真值（未绑定就不提），`·` 只在两条都在时才出现——
          // 拼一句「· 说一条」出来比不给提示更糟。
          // 两条全局键只在首次空态给：筛不到东西时讲快捷键是答非所问。
          footnote={filtering ? undefined : shortcutFootnote}
        />
      ) : (
        <FolderedItems
          items={notes}
          rootDir={folders.rootDir ?? ""}
          dirs={folders.dirs}
          listClassName="luna-inbox__items"
          // 多选的键盘契约（↑↓ / Shift+↑↓ / Home·End / Esc / Delete）只挂在最外层那个
          // `<ul>` 上，与另外四条同样可多选的清单同一处。
          rootListProps={{
            role: "listbox",
            "aria-multiselectable": true,
            "aria-label": t("draft.listLabel"),
            ...selection.containerProps,
          }}
          onVisibleRows={onVisibleRows}
          groupRootItems={(rootItems) =>
            bucketDrafts(rootItems, now).map((bucket) => ({
              id: bucket.id,
              label: t(`common:timeBucket.${bucket.id}`),
              items: bucket.items,
            }))
          }
          creating={creatingFolder}
          onCreateFolder={folders.createFolder}
          onCreateCancel={() => setCreatingFolder(false)}
          onRenameFolder={folders.renameFolder}
          onDeleteFolder={folders.deleteFolder}
          onMoveInto={folders.moveInto}
          renderItem={(note, beginDrag) => (
                <li key={note.path} role="none" className="luna-inbox__row">
                  <button
                    type="button"
                    className="luna-inbox__row-open"
                    data-stale={note.stale || undefined}
                    // 与另外四条可多选的清单同一套：`option` + `aria-selected` 说「这一批被选中」，
                    // `aria-current` 说「这一条正开着」。前者由 `rowProps` 贴，方向键与
                    // 连选由容器兑现。
                    role="option"
                    {...selection.rowProps(note.path)}
                    aria-current={selectedPath === note.path || undefined}
                    title={note.path}
                    // 拖进分节：选中集里含本条就整批一起拖（与文件树同一规则）。
                    onPointerDown={(event) =>
                      beginDrag(
                        event,
                        selection.contains(note.path) ? selection.ordered : [note.path],
                      )
                    }
                    onClick={(event) => {
                      // Cmd/Ctrl 加选、Shift 连选都不打开：每加选一条就把主区切过去，
                      // 用户根本选不满三条（PRD-06-059）。
                      if (selection.click(note.path, event)) {
                        onSelect(note.path);
                      }
                    }}
                    onContextMenu={(event) => {
                      // 右键落在选中集里就保持整批，落在集外则单选它——菜单动作与看到的
                      // 高亮必须是同一批对象。
                      const targets = selection.targetsFor(note.path);
                      selection.contextClick(note.path);
                      if (!selection.contains(note.path)) {
                        onSelect(note.path);
                      }
                      onItemContextMenu(event, note, targets);
                    }}
                  >
                    {/*
                      行图标的判据是 `has_media`，也就是「这条有原声可以听」，不是「当初
                      是说的还是敲的」。后者文件里没有记，为一枚图标新增持久化字段是让
                      存储替呈现背债；而「留没留下录音」既有索引早就知道，且比来源更有用
                      ——用户想知道的是能不能回去听一遍。
                    */}
                    {note.has_media ? (
                      <MicIcon size={14} className="luna-inbox__row-icon" />
                    ) : (
                      <StickyNoteIcon size={14} className="luna-inbox__row-icon" />
                    )}
                    {/*
                      行的两层：第一行是身份（标题），第二行是这条内容的元信息（预览 +
                      待办 + 时长）。

                      内容元信息不许挤在标题那一行——它们都是 `flex-shrink: 0`，而标题
                      能收缩到 0，于是加一枚徽标就把标题整个挤没（真机 2026-09-09 复验：
                      加上「待办 0/2」之后第一行只剩徽标与时龄，标题一个字都不剩）。标题
                      是这一行的身份，最不该被牺牲的就是它。
                    */}
                    <span className="luna-inbox__row-body">
                      <span className="luna-inbox__row-title">{note.title}</span>
                      {/* 第二行恒在：时龄与「AI 已整理」角标也住这里——它们此前与行主体同层且都
                          不收缩，英文的「24 minutes ago」加上 150% 字号就把主体挤成零宽，标题与
                          预览一起消失（真机 2026-10-06，窄窗）。第二行里只有预览会收缩。 */}
                      <small className="luna-inbox__row-meta">
                        {note.preview && (
                          <span className="luna-inbox__row-preview">{note.preview}</span>
                        )}
                        {/* 待办进度（PRD-06-058 G）：AI 提炼写下的与用户自己敲的复选框
                            都算，它们是同一种正文事实。统计在 Rust，前端不解析正文。
                            全部勾完时换一种说法而不是显示 5/5——那一刻用户要看的是
                            「这条可以收了」，不是再数一遍。 */}
                        {note.todo_total > 0 && (
                          <span
                            className="luna-inbox__row-todos"
                            data-done={note.todo_open === 0 || undefined}
                          >
                            {note.todo_open === 0
                              ? t("draft.todoAllDone")
                              : t("draft.todoProgress", {
                                  // 变量名是「已完成」——`todo_open` 是还没勾的那些，
                                  // 直接把它填进进度里，读出来的意思正好是反的。
                                  done: note.todo_total - note.todo_open,
                                  total: note.todo_total,
                                })}
                          </span>
                        )}
                        {/* 有原声就报时长：一条 8 秒的随口一说与一条 3 分钟的口述，
                            处理它们的心理成本完全不同，而这个数字既有索引里就有。 */}
                        {note.has_media && (note.duration_ms ?? 0) > 0 && (
                          <span className="luna-inbox__row-duration">
                            {formatClock(note.duration_ms)}
                          </span>
                        )}
                        {/* AI 已整理（PRD-06-058 C′）：判定是 Rust 下发的布尔，前端不读
                            frontmatter。用角标而不是文字——名字进 title 与读屏名。 */}
                        {note.refined && (
                          <span
                            className="luna-inbox__row-refined"
                            title={t("draft.refined")}
                            aria-label={t("draft.refined")}
                          >
                            <SparklesIcon size={11} />
                          </span>
                        )}
                        <span className="luna-inbox__row-age">
                          {formatRelativeTime(note.modified_ms, now, i18n.language)}
                        </span>
                      </small>
                    </span>
                  </button>
                  <div className="luna-inbox__row-actions">
                    <IconButton
                      label={t("draft.settle")}
                      icon={<BookOpenIcon size={13} />}
                      density="sm"
                      onClick={() => onSettle(note.path)}
                    />
                    <IconButton
                      label={t("distill.generate")}
                      icon={<SparklesIcon size={13} />}
                      density="sm"
                      onClick={() => onDistill(note.path)}
                    />
                    <IconButton
                      label={t("draft.openInNotes")}
                      icon={<PencilIcon size={13} />}
                      density="sm"
                      onClick={() => onOpenInNotes(note.path)}
                    />
                  </div>
                </li>
          )}
        />
      )}
      {footer}
    </section>
  );
}
