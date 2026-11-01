import type { CSSProperties } from "react";

/**
 * React only receives runtime values through CSS custom properties. Layout and
 * visual decisions stay in the owning stylesheet, so components never acquire
 * a second inline styling system.
 */
export type CssVariables = Record<`--${string}`, string | number | undefined>;

export function cssVariables(values: CssVariables): CSSProperties {
  return values as CSSProperties;
}
