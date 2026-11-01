import { useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";

/** Copy the exact value; feedback belongs to that value, never a previous one. */
export function CopyValueButton({ value, label, copiedLabel }: { value: string; label: string; copiedLabel?: string }) {
  const { t } = useTranslation("common");
  const [feedback, setFeedback] = useState<{ value: string; ok: boolean } | null>(null);
  async function copy() {
    const result = await commands.popupCopy(value);
    setFeedback({ value, ok: result.status === "ok" });
  }
  return <>
    <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => void copy()}>{label}</button>
    {feedback?.value === value ? <span role="status">
      {feedback.ok ? copiedLabel ?? t("menu.copied") : t("details.copy_failed")}
    </span> : null}
  </>;
}
