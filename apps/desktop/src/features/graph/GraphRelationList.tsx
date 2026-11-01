/**
 * 知识网络的关系列表（PRD-16-052 / K7）：与画布**同一份** DTO 的可键盘操作替身。
 *
 * 每个节点一行（原生 `<details>`，展开键盘可达），展开列出它的每条关系——链接到 / 被链接 /
 * 带的标签 / 带这个标签的笔记——每条都能直接打开。只转述边的种类，不标「支持」「因果」。
 * 画布画不出来（上下文不可用）时，宿主直接落到这里，节点照样找得到、打得开。
 */

import { useTranslation } from "react-i18next";
import type { GraphEdgeDto, GraphNodeDto } from "../../ipc/bindings";
import { relationsOf } from "./graph-sim";

export interface GraphRelationListProps {
  nodes: readonly GraphNodeDto[];
  edges: readonly GraphEdgeDto[];
  onOpen: (node: Pick<GraphNodeDto, "id" | "kind">) => void;
}

export function GraphRelationList({ nodes, edges, onOpen }: GraphRelationListProps) {
  const { t } = useTranslation("workspace");
  const kindOf = new Map(nodes.map((node) => [node.id, node.kind]));
  const ordered = [...nodes].sort((a, b) => b.degree - a.degree || a.label.localeCompare(b.label));
  return (
    <ul className="luna-graph-list" aria-label={t("graph.list.title")}>
      {ordered.map((node) => {
        const relations = relationsOf(node.id, nodes, edges);
        return (
          <li key={node.id}>
            <details>
              <summary className="luna-graph-list__summary" tabIndex={0}>
                <span className="luna-graph-list__label">{node.label}</span>
                <span className="luna-graph-list__count">
                  {t("graph.list.relations", { count: relations.length })}
                </span>
              </summary>
              <div className="luna-graph-list__body">
                <button
                  type="button"
                  tabIndex={0}
                  className="luna-btn luna-btn--secondary luna-btn--sm"
                  onClick={() => onOpen(node)}
                >
                  {t(
                    node.kind === "tag"
                      ? "graph.list.openTag"
                      : node.kind === "topic"
                        ? "graph.list.openTopic"
                        : "graph.list.open",
                  )}
                </button>
                {relations.length > 0 && (
                  <ul className="luna-graph-list__relations">
                    {relations.map((relation) => (
                      <li key={`${relation.kind}:${relation.id}`}>
                        <span className="luna-graph-list__kind">
                          {t(`graph.list.kind.${relation.kind}`)}
                        </span>
                        <button
                          type="button"
                          tabIndex={0}
                          className="luna-graph-list__target"
                          onClick={() =>
                            onOpen({ id: relation.id, kind: kindOf.get(relation.id) ?? "note" })
                          }
                        >
                          {relation.label}
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </details>
          </li>
        );
      })}
    </ul>
  );
}
