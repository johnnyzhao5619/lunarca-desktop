/**
 * 创作空间看板的纯函数（ADR-162 / PRD-19-003）：把 Rust 给出的列序与卡片排成列、
 * 在拖拽落点 id 与阶段之间互转。
 *
 * **阶段的判定不在这里**：哪一篇是项目、它在哪一列（含「无键 / 不认识的词 → 选题」）都由
 * Rust 算好下发（`creator_board`）；列序也由 Rust 给，前端不自带一份阶段清单。
 */

import type { ProjectCardDto, ProjectStageDto } from "../../ipc/bindings";

/** 看板列在拖拽落点上的 id 前缀（`data-drop-id` 是全应用共用的落点属性）。 */
const STAGE_DROP_PREFIX = "creator-stage:";

/**
 * 一列 = 一个阶段 + 显示在这一列的卡片（保持 Rust 给的顺序：主页修改时间倒序）。
 *
 * 按卡片的 `column` 分组而不是 `stage`：停在隐藏列上的项目由 Rust 放进它前面最近的显示列
 * （`display_column`），前端不自己再判一遍。
 */
export interface BoardColumn {
  stage: ProjectStageDto;
  cards: ProjectCardDto[];
}

export function columnsOf(
  stages: readonly ProjectStageDto[],
  projects: readonly ProjectCardDto[],
): BoardColumn[] {
  return stages.map((stage) => ({
    stage,
    cards: projects.filter((project) => project.column === stage),
  }));
}

/** 阶段 → 列的落点 id。 */
export function stageDropId(stage: ProjectStageDto): string {
  return `${STAGE_DROP_PREFIX}${stage}`;
}

/** 落点 id → 阶段；不是看板列（或不在当前列序里）→ `null`，松手即取消。 */
export function stageOfDropId(
  dropId: string,
  stages: readonly ProjectStageDto[],
): ProjectStageDto | null {
  if (!dropId.startsWith(STAGE_DROP_PREFIX)) {
    return null;
  }
  const id = dropId.slice(STAGE_DROP_PREFIX.length);
  return stages.find((stage) => stage === id) ?? null;
}

/** 卡片是不是借住在别的列里（它的阶段那一列被隐藏了）——是的话卡片上要标出真实阶段。 */
export function isDisplacedCard(card: ProjectCardDto): boolean {
  return card.column !== card.stage;
}

/** 阶段名的 i18n key（`workspace` 命名空间）。 */
export function stageLabelKey(stage: ProjectStageDto): string {
  return `creator.stage.${stage}`;
}
