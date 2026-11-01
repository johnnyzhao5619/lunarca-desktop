/**
 * 真实 Vault 树上的**纯视图计算**：数一数、临时藏一藏、把某条路径的祖先摊开。
 *
 * 它服务的是**一棵真实树**（ADR-087 ①：全部笔记只渲染 Rust 下发的真实拓扑），与已退役的
 * 「全部 / 知识 / 资料」三态分区无关——那套分区连同 `note-tree-partition.ts` 一起删除了，
 * 这三件事跟着自己的职责搬到这里。
 *
 * 判据全部来自 Rust 已判定的布尔（`NoteTreeDto.draft`），本模块不认识成熟度的任何取值
 *（ADR-003）。
 */

import type { NoteTreeDto } from "../../ipc/bindings";

/** 这一片里有多少个文件（目录不计）——树头那枚件数徽标。 */
export function countEntries(nodes: readonly NoteTreeDto[]): number {
  return nodes.reduce((total, node) => total + (node.is_dir ? countEntries(node.children) : 1), 0);
}

/**
 * 记录态笔记行的**文件级**过滤：把记录态的笔记行从树里摘掉（PRD-06-052）。
 *
 * **只摘文件行，目录一律保留**（连同因此变空的目录）：目录恒非记录态（判定在 Rust），
 * 而把「只装着草稿」的文件夹一并藏掉会让文件树对磁盘说谎——它还在那儿，用户下一步想
 * 往里新建就找不到落点了。开关表达的是「暂时别让草稿分散注意力」，不是「删掉这一片」。
 *
 * **只改变这一次看到什么**：不落盘、不移动文件、不生成虚拟根——树自始至终是磁盘的
 * 如实呈现（PRD-06-048 ①）。
 *
 * `draft` 是 Rust 已判定的布尔（`NoteTreeDto.draft`）——前端不认识任何取值（ADR-003）。
 */
export function hideDraftNodes(nodes: readonly NoteTreeDto[]): NoteTreeDto[] {
  const kept: NoteTreeDto[] = [];
  for (const node of nodes) {
    if (!node.is_dir) {
      if (!node.draft) {
        kept.push(node);
      }
      continue;
    }
    kept.push({ ...node, children: hideDraftNodes(node.children) });
  }
  return kept;
}

/**
 * 这一片里有没有记录态笔记——「隐藏待整理」开关的**渲染条件**。
 *
 * 一个没有可隐藏对象的开关只是占位的噪音。判据恒按**未过滤**的树算，否则开一次就
 * 再也关不回来了。
 */
export function containsDraft(nodes: readonly NoteTreeDto[]): boolean {
  return nodes.some((node) => (node.is_dir ? containsDraft(node.children) : node.draft));
}

/**
 * 某条 Vault 相对路径的祖先目录链（由外向内，不含它自己）。
 *
 * 「在全部笔记中显示」要真的**看得见**（PRD-07-006：各查询投影打开的是同一篇 Markdown，
 * 且能回到全部笔记定位它）：投影把真实 path 交过来时，它的祖先目录可能全是收起的——
 * 只把那一行标成选中，用户在屏幕上什么都不会发生，而那一行根本没被渲染出来。
 *
 * **纯字符串推导，不查树**：路径就是归属（笔记即文件），故这里不需要也不应该再去
 * 树里找一遍——树还没到、或那一片还没展开时，答案也必须是同一个。
 */
export function ancestorDirsOf(path: string): string[] {
  const parts = path.split("/").filter(Boolean);
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join("/"));
}

/**
 * 树里的**全部文件夹**（含库根），按屏幕顺序、带层级——「移动到 ▸」的候选。
 *
 * **候选取自真实的树，不是某个归档目录**：文件树管的是整个库，把它的移动目标限死在
 * 一个类型归档目录里（四条清单的做法）会让"从 Notes 移到 Recordings"这种再正常不过的
 * 事做不到。折叠与否不影响候选——菜单不是那棵树，用户要移到哪儿与他此刻展开了什么无关。
 */
export function dirChoices(
  nodes: readonly NoteTreeDto[],
  sort: (children: readonly NoteTreeDto[]) => NoteTreeDto[],
): { path: string; depth: number; name: string }[] {
  const out: { path: string; depth: number; name: string }[] = [{ path: "", depth: 0, name: "" }];
  const walk = (children: readonly NoteTreeDto[], depth: number) => {
    for (const node of sort(children)) {
      if (!node.is_dir) {
        continue;
      }
      out.push({ path: node.path, depth, name: node.name });
      walk(node.children, depth + 1);
    }
  };
  walk(nodes, 1);
  return out;
}

/**
 * 此刻**屏幕上真的有那一行**的路径，按渲染顺序（多选连选与方向键都按它取区间）。
 *
 * **收起的目录里那些行不在其中**：多选恒在可见行上收敛（`note-selection.ts`），
 * 否则「删除 5 项」会动到用户此刻根本看不见的对象。排序口径与渲染同一个函数
 * （`sortTreeChildren`），不能在这里另排一遍——两边一旦不同序，Shift 连选选中的
 * 区间就与屏幕上那一段对不上。
 */
export function visibleRowPaths(
  nodes: readonly NoteTreeDto[],
  expandedDirs: ReadonlySet<string>,
  sort: (children: readonly NoteTreeDto[]) => NoteTreeDto[],
): string[] {
  const out: string[] = [];
  const walk = (children: readonly NoteTreeDto[]) => {
    for (const node of sort(children)) {
      out.push(node.path);
      if (node.is_dir && expandedDirs.has(node.path)) {
        walk(node.children);
      }
    }
  };
  walk(nodes);
  return out;
}
