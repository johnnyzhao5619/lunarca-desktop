/**
 * 文件树上方的「书签」区（PRD-06-006，笔记库对标轮 V5b）。
 *
 * 书签随库存放（`.luna/bookmarks.json`，ADR-140 ①），增删、跟随改名、只列还在的条目全在 Rust；
 * 这里只取数与呈现。没有书签时整块不出现——空着的「书签」标题只是噪音。标题沿用文件夹
 * 分组的形态（chevron + 名字 + 条数），条目沿用文件树行的样子：它们本来就是树里的东西。
 */

import { useCallback, useEffect, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { commands, events, type VaultBookmarkDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { ChevronDownIcon, ChevronRightIcon, FileIcon, FolderIcon } from "@lunarca/editor-core/Icon";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu } from "../../lib/context-menu";
import { vaultErrorText } from "../../lib/vault-error";

export interface VaultBookmarks {
  list: readonly VaultBookmarkDto[];
  isBookmarked: (path: string) => boolean;
  /** 设定书签态；失败返回错误码串（成功返回 `null`）。 */
  setBookmarked: (path: string, bookmarked: boolean) => Promise<string | null>;
}

/** 书签：挂载、切库与库有任何变化时重取（被删的条目要从列表里消失，还原后要回来）。 */
export function useVaultBookmarks(): VaultBookmarks {
  const [list, setList] = useState<readonly VaultBookmarkDto[]>([]);
  const load = useCallback(() => {
    void commands.bookmarks().then((result) => {
      setList(result.status === "ok" ? result.data : []);
    });
  }, []);
  useEffect(() => {
    load();
    const unlisten = events.vaultSwitched.listen(load);
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [load]);
  useVaultReload(load);
  const setBookmarked = useCallback(async (path: string, bookmarked: boolean) => {
    const result = await commands.setBookmark(path, bookmarked);
    if (result.status === "error") {
      return result.error;
    }
    setList(result.data);
    return null;
  }, []);
  return {
    list,
    isBookmarked: (path) => list.some((bookmark) => bookmark.path === path),
    setBookmarked,
  };
}

function displayName(path: string): string {
  const name = path.split("/").pop() ?? path;
  return name.replace(/\.md$/i, "");
}

export function BookmarksSection({
  bookmarks,
  onOpen,
  onStatus,
}: {
  bookmarks: VaultBookmarks;
  /** 打开一条书签：笔记 / 附件按树里单击的方式打开，文件夹在树里定位并展开。 */
  onOpen: (bookmark: VaultBookmarkDto) => void;
  onStatus: (message: string) => void;
}) {
  const { t } = useTranslation("workspace");
  const contextMenu = useContextMenu();
  const [collapsed, setCollapsed] = useState(false);
  if (bookmarks.list.length === 0) {
    return null;
  }
  const Chevron = collapsed ? ChevronRightIcon : ChevronDownIcon;

  function openMenu(event: ReactMouseEvent, bookmark: VaultBookmarkDto) {
    // 原生菜单由 `contextMenu.open` 拦；这里先拦会让宿主把这次右键当成「已有人认领」而不弹。
    event.stopPropagation();
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "removeBookmark",
          label: t("bookmarks.remove"),
          onSelect: () => {
            void bookmarks.setBookmarked(bookmark.path, false).then((error) => {
              if (error !== null) onStatus(vaultErrorText(error));
            });
          },
        },
      ]),
    );
  }

  return (
    <section className="luna-bookmarks" aria-label={t("bookmarks.title")}>
      <button
        type="button"
        className="luna-folder-group__head"
        aria-expanded={!collapsed}
        onClick={() => setCollapsed((current) => !current)}
      >
        <Chevron size={12} className="luna-folder-group__chevron" />
        <span className="luna-folder-group__name">{t("bookmarks.title")}</span>
        <span className="luna-folder-group__count">{bookmarks.list.length}</span>
      </button>
      {!collapsed && (
        <ul className="luna-bookmarks__items">
          {bookmarks.list.map((bookmark) => (
            <li key={bookmark.path}>
              <button
                type="button"
                className="luna-note-tree__item-btn"
                title={bookmark.path}
                onClick={() => onOpen(bookmark)}
                onContextMenu={(event) => openMenu(event, bookmark)}
              >
                {bookmark.is_dir ? (
                  <FolderIcon size={14} className="luna-note-tree__icon" />
                ) : (
                  <FileIcon size={14} className="luna-note-tree__icon" />
                )}
                <span className="luna-note-tree__item-name">{displayName(bookmark.path)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
