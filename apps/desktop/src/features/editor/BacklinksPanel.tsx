// 反向链接面板（PRD-06-031 P1）：列出指向当前笔记的 note refs。

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type NoteRefDto } from "../../ipc/bindings";
import { notePathLabel } from "../../lib/note-path";
import { LinkIcon } from "../../components/ui/Icon";
import { EmptyState } from "../../components/ui/EmptyState";
import { useIndexStatus } from "../../hooks/use-index-status";

interface BacklinksPanelProps {
  path: string | null;
  onOpenPath?: (path: string) => void;
  showHeading?: boolean;
}

export function BacklinksPanel({ path, onOpenPath, showHeading = true }: BacklinksPanelProps) {
  const { t } = useTranslation("workspace");
  const [backlinks, setBacklinks] = useState<NoteRefDto[]>([]);
  const [loading, setLoading] = useState(false);
  // 开库后的增量扫描没做完时反链可能不全：明说，并在扫描结束时重取（PRD-06-004 ④）。
  const indexStatus = useIndexStatus();
  const indexing = indexStatus !== null;

  useEffect(() => {
    if (!path) {
      setBacklinks([]);
      return;
    }
    setLoading(true);
    void commands.listBacklinks(path).then((res) => {
      if (res.status === "ok") {
        setBacklinks(res.data);
      } else {
        setBacklinks([]);
      }
      setLoading(false);
    });
  }, [path, indexing]);

  if (!path) {
    return <EmptyState icon={<LinkIcon size={24} />} title={t("panel.noSelection")} />;
  }

  if (loading) {
    return <p className="luna-empty">{t("common:status.loading")}</p>;
  }

  return (
    <div>
      {showHeading ? <h3>{t("panel.backlinks")}</h3> : null}
      {indexStatus && (
        <p className="luna-empty" role="status">
          {t("common:status.indexingPartial", { indexed: indexStatus.indexed, total: indexStatus.total })}
        </p>
      )}
      {backlinks.length === 0 ? (
        <EmptyState icon={<LinkIcon size={24} />} title={t("panel.empty")} />
      ) : (
        <ul className="luna-panel-link-list">
          {backlinks.map((ref) => (
            <li key={ref.path}>
              <button
                type="button"
                className="luna-panel-link-list__item"
                title={ref.path}
                onClick={() => {
                  if (onOpenPath) onOpenPath(ref.path);
                }}
              >
                <span>{notePathLabel(ref.path)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
