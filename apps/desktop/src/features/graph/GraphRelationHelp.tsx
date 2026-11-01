import { useTranslation } from "react-i18next";
import type { GraphMode } from "./use-graph-controller";

/** 两种呈现共用关系语义；限制留在摘要里，详细解释按需展开。 */
export function GraphRelationHelp({ mode }: { mode: GraphMode }) {
  const { t } = useTranslation("workspace");
  return (
    <details className="luna-graph__relation-help">
      <summary tabIndex={0}>{t(mode === "topics" ? "graph.topicRelationSummary" : "graph.relationSummary")}</summary>
      <p>{t(mode === "topics" ? "graph.topicLegend" : "graph.legend")}</p>
    </details>
  );
}
