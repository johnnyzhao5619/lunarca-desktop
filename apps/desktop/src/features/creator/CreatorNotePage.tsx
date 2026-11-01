/**
 * 创作空间里的一篇非项目笔记（灵感条目，ADR-165 ⑥）：就地编辑，不跳去「笔记」空间。
 *
 * 页头与项目工作区同一种写法：面包屑回到它来的地方（灵感库），「在笔记中打开」与「更多」是次要
 * 动作；「更多」是这篇笔记的笔记菜单（重命名、删除……，与侧栏那一行同一份，ADR-167 ①）。
 */

import { useEffect, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { IconButton } from "../../components/ui/IconButton";
import { EllipsisIcon } from "../../components/ui/Icon";
import { CreatorPageHeader } from "./CreatorPage";

export interface CreatorNotePageProps {
  path: string;
  /** 面包屑：回灵感库。 */
  onBack: () => void;
  renderEditor: (path: string, openPath: (path: string) => void) => ReactNode;
  /** 编辑器里点到的链接：交给宿主决定在哪打开。 */
  onOpenPath: (path: string) => void;
  onOpenInNotes: (path: string) => void;
  onActiveDocument: (path: string | null) => void;
  /** 「更多」：这篇笔记的笔记菜单（宿主组装）。 */
  onMore: (event: ReactMouseEvent, path: string) => void;
}

function stemOf(path: string): string {
  const name = path.split("/").pop() ?? path;
  return name.endsWith(".md") ? name.slice(0, -3) : name;
}

export function CreatorNotePage({
  path,
  onBack,
  renderEditor,
  onOpenPath,
  onOpenInNotes,
  onActiveDocument,
  onMore,
}: CreatorNotePageProps) {
  const { t } = useTranslation("workspace");

  useEffect(() => {
    onActiveDocument(path);
  }, [path, onActiveDocument]);
  useEffect(() => () => onActiveDocument(null), [onActiveDocument]);

  return (
    <section className="luna-creator" aria-labelledby="creator-note-title">
      <CreatorPageHeader
        crumb={{ label: t("creator.swipeLibrary.title"), onClick: onBack }}
        title={stemOf(path)}
        titleId="creator-note-title"
        secondary={
          <>
            <button type="button" className="luna-btn luna-btn--secondary" onClick={() => onOpenInNotes(path)}>
              {t("creator.page.openInNotes")}
            </button>
            <IconButton
              label={t("creator.page.more")}
              icon={<EllipsisIcon size={16} />}
              aria-haspopup="menu"
              onClick={(event) => onMore(event, path)}
            />
          </>
        }
      />
      <div className="luna-creator-page__editor">{renderEditor(path, onOpenPath)}</div>
    </section>
  );
}
