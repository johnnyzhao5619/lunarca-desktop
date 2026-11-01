import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import type { HtmlPasteDto } from "../../ipc/bindings";
import { useState } from "react";
import type { TablePastePreview } from "./table-edit";
import type { PasteConversionChoice } from "./paste-assets";

export function PastePreviewDialog({ preview, plain, tableTarget, onResolve }: {
  preview: HtmlPasteDto; plain: string; tableTarget?: TablePastePreview; onResolve: (choice: PasteConversionChoice) => void;
}) {
  const { t } = useTranslation("editor");
  const ref = useDialogBehavior<HTMLDivElement>({ onClose: () => onResolve(null) });
  const [headerChoice, setHeaderChoice] = useState<HtmlPasteDto | null>(null);
  const headerFirst = headerChoice === preview;
  const markdown = headerFirst ? preview.header_markdown ?? preview.markdown : preview.markdown;
  return <div className="luna-overlay" role="presentation" onClick={() => onResolve(null)}>
    <div ref={ref} className="luna-dialog" role="dialog" aria-modal="true" aria-label={t(tableTarget ? "paste.tableFillTitle" : "paste.previewTitle")} onClick={(event) => event.stopPropagation()}>
      <header className="luna-dialog__header"><h3>{t(tableTarget ? "paste.tableFillTitle" : "paste.previewTitle")}</h3><p>{t(tableTarget ? "paste.tableFillHint" : "paste.previewHint")}</p></header>
      {tableTarget ? <div>
        <p>{t("paste.tableFillRange", { ...tableTarget })}</p>
        <p>{t("paste.tableFillChanges", { ...tableTarget })}</p>
      </div> : null}
      <ul>{preview.warnings.filter((warning) => !headerFirst || warning !== "missing_header").map((warning) => <li key={warning}>{t(`paste.warning.${warning}`)}</li>)}</ul>
      {preview.header_markdown !== null && !tableTarget ? <label>
        <input type="checkbox" checked={headerFirst} onChange={(event) => setHeaderChoice(event.target.checked ? preview : null)} />
        {t("paste.firstRowHeader")}
      </label> : null}
      <pre className="luna-paste-preview__source">{markdown}</pre>
      <div className="luna-dialog__actions">
        <button type="button" className="luna-btn luna-btn--secondary" onClick={() => onResolve(null)}>{t("paste.cancel")}</button>
        <button type="button" className="luna-btn luna-btn--secondary" disabled={!plain} onClick={() => onResolve({ kind: "plain", text: plain })}>{t("paste.insertPlain")}</button>
        <button type="button" className="luna-btn luna-btn--primary" onClick={() => onResolve({ kind: "converted", markdown })}>{t(tableTarget ? "paste.fillTable" : "paste.insertConverted")}</button>
      </div>
    </div>
  </div>;
}
