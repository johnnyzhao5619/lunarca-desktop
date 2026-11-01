/**
 * 公式（阅读面 / 对话 / 静态导出共用）：KaTeX 已加载就同步出排版结果（静态导出依赖这一点，
 * 导出前会先 `loadKatex()`），没加载先显示 TeX 原文，加载完再换上。
 */

import { useEffect, useState } from "react";
import { loadKatex, renderMathToString } from "../../lib/math-render";

export function MathView({ tex, display }: { tex: string; display: boolean }) {
  const [html, setHtml] = useState(() => renderMathToString(tex, display));

  useEffect(() => {
    let active = true;
    const ready = renderMathToString(tex, display);
    setHtml(ready);
    if (ready === null) {
      void loadKatex().then(() => {
        if (active) setHtml(renderMathToString(tex, display));
      });
    }
    return () => {
      active = false;
    };
  }, [tex, display]);

  const Tag = display ? "div" : "span";
  const className = display ? "luna-math luna-math--block" : "luna-math";
  if (html === null) return <Tag className={className}>{tex}</Tag>;
  // KaTeX 生成并转义输入，`trust: false` 不引用外部资源（见 lib/math-render.ts）。
  return <Tag className={className} dangerouslySetInnerHTML={{ __html: html }} />;
}
