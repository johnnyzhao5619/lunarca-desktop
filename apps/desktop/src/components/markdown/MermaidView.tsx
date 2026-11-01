/**
 * Mermaid 图表（阅读面 / 对话 / 静态导出共用）：渲染过的同主题同源码同步给出（静态导出前会先
 * 预热），否则先显示源码，画好再换上；语法错误显示 Mermaid 的说明与源码。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { mermaidIfRendered, renderMermaid } from "../../lib/mermaid-render";

export function MermaidView({ code }: { code: string }) {
  const { t } = useTranslation("common");
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
    // Mermaid strict 模式的输出（DOMPurify 清洗、无脚本与外链，见 lib/mermaid-render.ts）。
    return <div className="luna-mermaid" dangerouslySetInnerHTML={{ __html: svg }} />;
  }
  return (
    <div className="luna-mermaid luna-mermaid--pending">
      {problem !== null && <p className="luna-field-error">{t("markdown.diagramError", { message: problem })}</p>}
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  );
}
