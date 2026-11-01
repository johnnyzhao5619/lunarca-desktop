import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { InlineDetails } from "./InlineDetails";
import { CopyValueButton } from "./CopyValueButton";

/** Show the actual DTO path, with a compact preview and a copyable full value. */
export function PathDetails({ path, preview = true, children }: {
  path: string;
  preview?: boolean;
  children?: ReactNode;
}) {
  const { t } = useTranslation("common");
  return (
    <div className="luna-inline-details__location">
      {preview ? <span className="luna-inline-details__preview" title={path}>{path}</span> : null}
      <InlineDetails summary={t("details.path")}>
        <PathValue path={path} />
        {children}
      </InlineDetails>
    </div>
  );
}

/** Full path and copy feedback, also usable inside a group of location details. */
export function PathValue({ path }: { path: string }) {
  const { t } = useTranslation("common");
  return (
    <div className="luna-inline-details__body">
        <p className="luna-inline-details__path">{path}</p>
        <div className="luna-inline-details__actions">
          <CopyValueButton value={path} label={t("menu.copyPath")} copiedLabel={t("menu.copiedPath")} />
        </div>
    </div>
  );
}
