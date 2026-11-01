// 「这一篇的标签」（PRD-07-031）：当前对象有哪些标签，点一个跳到「知识库 → 标签」。
//
// **这里不是标签树的第二个副本**（ADR-088）：全库标签树 + 该标签下的笔记列表是
// 「知识库 → 标签」那一整个二级分区的事。本格只回答「这一篇有哪些标签」——它是对象级
// 事实，与「笔记信息」同源同一次取数（`note_facts`），不另算一份统计。

import { useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, type NoteTagEditDto } from "../../ipc/bindings";
import { EmptyState } from "../../components/ui/EmptyState";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon, TagsIcon } from "../../components/ui/Icon";
import { useNoteFacts } from "./use-note-facts";

interface NoteTagsPanelProps {
  path: string | null;
  /** 点标签 → 打开「知识库 → 标签」并选中它（与编辑器内联 `#tag` 点击同一入口）。 */
  onOpenTag: (tag: string) => void;
}

/**
 * 就地增删用户标签（ADR-135）：写不写、写在哪、为什么不能在这里删，全由 Rust `edit_note_tag`
 * 判定——只改笔记开头的 `tags`；写在正文里的 `#标签` 与 AI 标签如实说明出口，不偷偷改正文。
 */
function editNotice(
  t: (key: string, options?: Record<string, unknown>) => string,
  outcome: NoteTagEditDto,
  tag: string,
  add: boolean,
): string | null {
  switch (outcome.status) {
    case "written":
      return null;
    case "unchanged":
      return t(add ? "panel.noteTagsAlready" : "panel.noteTagsNotFound", { tag });
    case "inline_only":
      return t("panel.noteTagsInlineOnly", { tag });
    case "ai_only":
      return t("panel.noteTagsAiOnly", { tag });
    case "skipped":
      return t("panel.noteTagsSkipped", { reason: t(`tagRename.reason.${outcome.reason}`) });
    case "invalid_name":
      return t("tagRename.error.invalid_name");
  }
}

export function NoteTagsPanel({ path, onOpenTag }: NoteTagsPanelProps) {
  const { t } = useTranslation("workspace");
  const { facts, loading, notePath } = useNoteFacts(path);
  const [draft, setDraft] = useState("");
  const [notice, setNotice] = useState<string | null>(null);

  if (!notePath) {
    return <EmptyState icon={<TagsIcon size={24} />} title={t("panel.noSelection")} />;
  }
  if (loading && !facts) {
    return <p className="luna-empty">{t("common:status.loading")}</p>;
  }

  const edit = (tag: string, add: boolean) => {
    void commands.editNoteTag(notePath, tag, add).then((result) => {
      if (result.status !== "ok") {
        setNotice(t("panel.noteTagsFailed"));
        return;
      }
      setNotice(editNotice(t, result.data, tag, add));
      if (add && result.data.status === "written") setDraft("");
    });
  };

  const tags = facts?.tags ?? [];
  const removable = new Set((facts?.removable_tags ?? []).map((tag) => tag.toLowerCase()));

  return (
    <div className="luna-note-tags__panel">
      {tags.length === 0 ? (
        <EmptyState
          icon={<TagsIcon size={24} />}
          title={t("panel.noteTagsEmpty")}
          description={t("panel.noteTagsEmptyHint")}
        />
      ) : (
        <ul className="luna-note-tags">
          {tags.map((tag) => (
            <li key={tag} className="luna-note-tags__chip">
              <button
                type="button"
                className="luna-note-tags__item"
                title={t("panel.noteTagsOpen", { tag })}
                onClick={() => onOpenTag(tag)}
              >
                #{tag}
              </button>
              <IconButton
                density="sm"
                label={t(
                  removable.has(tag.toLowerCase()) ? "panel.noteTagsRemove" : "panel.noteTagsRemoveElsewhere",
                  { tag },
                )}
                icon={<CloseIcon size={10} />}
                onClick={() => edit(tag, false)}
              />
            </li>
          ))}
        </ul>
      )}
      <form
        className="luna-note-tags__add"
        onSubmit={(event) => {
          event.preventDefault();
          if (draft.trim()) edit(draft, true);
        }}
      >
        <input
          className="luna-input"
          value={draft}
          placeholder={t("panel.noteTagsAddPlaceholder")}
          aria-label={t("panel.noteTagsAddPlaceholder")}
          onChange={(event) => setDraft(event.target.value)}
        />
        <button type="submit" className="luna-btn luna-btn--secondary luna-btn--sm">
          {t("panel.noteTagsAdd")}
        </button>
      </form>
      {notice !== null && (
        <p className="luna-note-tags__notice" role="status">
          {notice}
        </p>
      )}
    </div>
  );
}
