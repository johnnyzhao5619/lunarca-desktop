// 外链面板（PRD-06-031 P1）：列出当前笔记指向的 note refs。
//
// 断链在这里**呈现为不可点的「未创建」**，而不是一个点下去只会弹错误的按钮：
// 「点了才失败」是本仓库反复修的同一类缺陷。存在性判定在 Rust（`exists`），前端只渲染。

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type OutgoingLinkDto } from "../../ipc/bindings";
import { notePathLabel } from "../../lib/note-path";
import { LinkIcon } from "@lunarca/editor-core/Icon";
import { EmptyState } from "../../components/ui/EmptyState";

interface OutgoingLinksPanelProps {
  path: string | null;
  onOpenPath?: (path: string) => void;
  showHeading?: boolean;
}

export function OutgoingLinksPanel({
  path,
  onOpenPath,
  showHeading = true,
}: OutgoingLinksPanelProps) {
  const { t } = useTranslation("workspace");
  const [outgoing, setOutgoing] = useState<OutgoingLinkDto[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!path) {
      setOutgoing([]);
      return;
    }
    setLoading(true);
    void commands.listOutgoingLinks(path).then((res) => {
      if (res.status === "ok") {
        setOutgoing(res.data);
      } else {
        setOutgoing([]);
      }
      setLoading(false);
    });
  }, [path]);

  if (!path) {
    return <EmptyState icon={<LinkIcon size={24} />} title={t("panel.noSelection")} />;
  }

  if (loading) {
    return <p className="luna-empty">{t("common:status.loading")}</p>;
  }

  return (
    <div>
      {showHeading ? <h3>{t("panel.outgoing")}</h3> : null}
      {outgoing.length === 0 ? (
        <EmptyState icon={<LinkIcon size={24} />} title={t("panel.empty")} />
      ) : (
        <ul className="luna-panel-link-list">
          {outgoing.map((ref) =>
            ref.exists ? (
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
            ) : (
              <li key={ref.path}>
                <span
                  className="luna-panel-link-list__item luna-panel-link-list__item--missing"
                  title={ref.path}
                >
                  <span>{notePathLabel(ref.path)}</span>
                  <em>{t("panel.linkMissing")}</em>
                </span>
              </li>
            ),
          )}
        </ul>
      )}
    </div>
  );
}
