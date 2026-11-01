import type { ReactNode } from "react";

/** Stable, named disclosure for instructions and actual details (PRD-09-085). */
export function InlineDetails({ summary, children }: { summary: string; children: ReactNode }) {
  return (
    <details className="luna-inline-details">
      <summary>{summary}</summary>
      <div className="luna-inline-details__body">{children}</div>
    </details>
  );
}
