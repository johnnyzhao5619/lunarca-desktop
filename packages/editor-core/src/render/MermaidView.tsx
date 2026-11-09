/**
 * Mermaid 图表（阅读面 / 对话 / 静态导出共用）。怎么画由宿主定（`MarkdownHost.diagrams`）：
 * - `inline`：渲染过的同主题同源码同步给出（静态导出前会先预热），否则先显示源码，画好再换上；
 * - `image`：交宿主画成图片，`<img>` 显示，图片地址在换图或卸载时交还宿主。
 * 两种都是：画好之前显示源码；画不出来显示原因与源码。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ImageDiagrams, MarkdownHost } from "../host";
import { HostHtml } from "./HostHtml";
import { mermaidIfRendered, renderMermaid } from "./mermaid-render";
import { useDiagramTheme } from "./use-diagram-theme";

export function MermaidView({ host, code }: { host: MarkdownHost; code: string }) {
  if (host.diagrams.kind === "image") return <ImageDiagram diagrams={host.diagrams} code={code} />;
  return <InlineDiagram host={host} code={code} />;
}

/** 还没画好（`problem` 为 null）或画不出来：源码照原样，画不出来时上面写明原因。 */
function DiagramSource({ code, problem }: { code: string; problem: string | null }) {
  const { t } = useTranslation("editor-core");
  return (
    <div className="luna-mermaid luna-mermaid--pending">
      {problem !== null && <p className="luna-field-error">{t("markdown.diagramError", { message: problem })}</p>}
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  );
}

function InlineDiagram({ host, code }: { host: MarkdownHost; code: string }) {
  const [svg, setSvg] = useState(() => mermaidIfRendered(code));
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setSvg(mermaidIfRendered(code));
    setProblem(null);
    void renderMermaid(code).then((result) => {
      if (!active) return;
      setSvg(result.svg);
      setProblem(result.syntaxProblem);
    });
    return () => {
      active = false;
    };
  }, [code]);

  if (svg !== null) {
    // Mermaid strict 模式的输出（DOMPurify 清洗、无脚本与外链，见 mermaid-render.ts）。
    return <HostHtml host={host} html={svg} as="div" className="luna-mermaid" />;
  }
  return <DiagramSource code={code} problem={problem} />;
}

type ImageState = { kind: "pending" } | { kind: "image"; url: string } | { kind: "error"; message: string };

function ImageDiagram({ diagrams, code }: { diagrams: ImageDiagrams; code: string }) {
  const { t } = useTranslation("editor-core");
  const theme = useDiagramTheme();
  const [state, setState] = useState<ImageState>({ kind: "pending" });

  useEffect(() => {
    let active = true;
    let release: (() => void) | null = null;
    setState({ kind: "pending" });
    void diagrams.render(code, theme).then((result) => {
      if (!active) {
        if (result.kind === "image") result.release();
        return;
      }
      if (result.kind === "image") {
        release = result.release;
        setState({ kind: "image", url: result.url });
      } else {
        setState(result);
      }
    });
    return () => {
      active = false;
      release?.();
    };
  }, [diagrams, code, theme]);

  if (state.kind === "image") {
    return (
      <div className="luna-mermaid">
        <img className="luna-mermaid__image" src={state.url} alt={t("markdown.diagramImage")} />
      </div>
    );
  }
  return <DiagramSource code={code} problem={state.kind === "error" ? state.message : null} />;
}
