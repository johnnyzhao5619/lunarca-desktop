/**
 * 已全库忽略的 AI 关系标签清单与恢复入口（PRD-16-051）。
 *
 * 忽略只改随库清单与派生索引（Rust `set_ai_tag_ignored`），笔记文件一字不动；这里只渲染
 * Rust 下发的清单与每个标签此刻压下的笔记数，恢复即原样回来。清单为空时整块不出现。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type IgnoredAiTagDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { ChevronDownIcon, ChevronRightIcon } from "@lunarca/editor-core/Icon";

export function IgnoredAiTags() {
  const { t } = useTranslation("workspace");
  const [ignored, setIgnored] = useState<IgnoredAiTagDto[]>([]);
  const [open, setOpen] = useState(false);

  const load = useCallback(() => {
    void commands.ignoredAiTags().then((result) => {
      setIgnored(result.status === "ok" ? result.data : []);
    });
  }, []);
  useEffect(load, [load]);
  useVaultReload(load);

  if (ignored.length === 0) return null;
  return (
    <div className="luna-tags-ignored">
      <button
        type="button"
        className="luna-tags-group__toggle"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        {open ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
        <span>{t("tags.ignoredAi", { count: ignored.length })}</span>
      </button>
      {open && (
        <ul className="luna-tags-ignored__list">
          {ignored.map((entry) => (
            <li key={entry.tag} className="luna-tags-ignored__row">
              <span className="luna-tags-ignored__tag">#{entry.tag}</span>
              <span className="luna-tags-ignored__count">
                {t("tags.ignoredAiNotes", { count: entry.note_count })}
              </span>
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                onClick={() => {
                  void commands.setAiTagIgnored(entry.tag, false).then(load);
                }}
              >
                {t("tags.restoreAiTag")}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
