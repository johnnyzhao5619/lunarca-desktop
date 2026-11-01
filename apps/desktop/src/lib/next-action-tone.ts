/**
 * 「下一步」行的语气词汇——共享布局原语 `.luna-next-row[data-tone]` 的 TS 对应物
 * （规则在 `styles/layout/next-action-row.css`）。
 *
 * 住在 `lib/` 而不是任一 feature 里：概览的三条行（装机 / 知识库脉搏 / 库体检）分属
 * `dashboard` 与 `knowledge` 两个功能模块，把它留在其中一边，另一边就得反向跨 feature
 * 引用——那正是结构门禁拦下的「功能模块存在循环依赖」。样式侧早就把这层认成跨 feature
 * 的零件（CSS 在 `styles/layout/`），类型侧跟上即可。
 *
 * 这里只放词汇。**谁**拿 accent 是概览的领域裁决，仍归
 * `features/dashboard/next-actions.ts`——同屏至多一条 accent。
 */

/** 行的语气：accent 是此刻的下一步（配实色主按钮），muted 是并列的次要项。 */
export type NextActionTone = "accent" | "muted";
