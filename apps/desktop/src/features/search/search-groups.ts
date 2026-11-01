/**
 * 搜索命中的三段分桶（ADR-024 来源轴 × ADR-086 成熟度轴）。
 *
 * 判据**全部由 Rust 下发**（`source_material` / `draft`）——前端既不判类型也不判成熟度
 * （ADR-003）。这里只做一件事：把一条扁平的命中列表切成呈现上的三段。
 *
 * **不重不漏由三态互斥保证**（`luna_vault::NoteStage`：来源轴优先，资料笔记上的
 * `stage` 键被忽略），故本模块不写任何"资料里别混进记录"的兜底——写了反而会掩盖
 * Rust 侧真出问题的那一天。
 *
 * 独立成模块（而不是留在 `SearchView` 的渲染体里）的理由与 `note-tree-view` 同：分桶
 * 口径是可以被无头测试锁住的**判定**，而它一旦漂移，界面上只表现为"某一篇跑到了另一个
 * 组里"——没有任何东西会因此变红。
 */

import type { SearchNoteDto } from "../../ipc/bindings";

export interface SearchHitGroups {
  /** 知识笔记：非资料**且**非记录。正常展开在最前，不折叠。 */
  knowledge: SearchNoteDto[];
  /** 记录：用户之言但未定稿。独立分组、**不降权**（PRD-06-052）。 */
  draft: SearchNoteDto[];
  /** 资料：录音 / 转写 / 对话（ADR-024）。降权呈现，默认折叠。 */
  material: SearchNoteDto[];
}

export function groupSearchHits(notes: readonly SearchNoteDto[]): SearchHitGroups {
  const groups: SearchHitGroups = { knowledge: [], draft: [], material: [] };
  for (const note of notes) {
    if (note.source_material) {
      groups.material.push(note);
    } else if (note.draft) {
      groups.draft.push(note);
    } else {
      groups.knowledge.push(note);
    }
  }
  return groups;
}
