/**
 * 清单/文件树的**多选代数**（PRD-06-059）——纯函数，一处实现，各清单共用。
 *
 * 之所以是独立模块而不是各列表自己的一段 `useState`：多选的难点从来不是"存一个 Set"，
 * 而是那几条容易各写各的规则——Shift 连选从哪儿到哪儿、Cmd 加选之后锚点在哪儿、右键
 * 落在选中集外该不该丢掉整批、列表变了之后已消失的行怎么办。任一条在两份清单里
 * 答得不一样，用户都会立刻察觉（"录音里能框、笔记里不能"正是这次要修的那类分叉）。
 *
 * **看不见的行不算选中**（`prunedSelection`）：选择集恒在"此刻可见的行"上收敛——
 * 折叠一个文件夹、切换筛选词、外部删掉一条，都会让某些行不再在场。留着它们的表现是
 * 「删除 5 项」实际动了 8 个对象，其中 3 个用户此刻根本看不见。
 */

/**
 * 选择集与它的两端。
 *
 * - `anchor` = Shift 连选的**起点**（最后一次单点/加选落在哪儿）；
 * - `lead` = 连选的**动端**，也是方向键下一步从哪儿走。
 *
 * **两端都必须在状态里，不能拿 DOM 焦点当动端**：第一版让 Shift+↓ 从"此刻聚焦的那
 * 一行"往下取，于是焦点没跟上（平台不给按钮焦点、或焦点被别处抢走）时，连按两次
 * Shift+↓ 每次都从锚点重新取同一段——选区停在原地。选择是**状态**，不是焦点的投影。
 */
export interface SelectionState {
  readonly paths: ReadonlySet<string>;
  readonly anchor: string | null;
  readonly lead: string | null;
}

/** 行点击时按下的修饰键（由调用方从事件里读出，纯函数不认识 DOM）。 */
export interface SelectionModifiers {
  /** Cmd/Ctrl：加选/去选这一行。 */
  toggle: boolean;
  /** Shift：从锚点连选到这一行。 */
  range: boolean;
}

export const EMPTY_SELECTION: SelectionState = { paths: new Set(), anchor: null, lead: null };

/** 单选（外部导航、创建后跳转、纯左键点击都落到这里）。 */
export function singleSelection(path: string | null): SelectionState {
  return path === null ? EMPTY_SELECTION : { paths: new Set([path]), anchor: path, lead: path };
}

/**
 * 点击后的新选择。`ordered` = 此刻可见行的顺序（连选按它取区间）。
 *
 * **Shift 连选是"替换成这一段"而不是"并上这一段"**（Finder / VS Code / Obsidian 同
 * 口径）：连选表达的是"我要的是从这儿到那儿"，把上一次的残留并进来会让用户拿到一个
 * 他没法解释的集合。要在既有集合上追加的是 Cmd。
 */
export function selectionAfterClick(
  current: SelectionState,
  path: string,
  modifiers: SelectionModifiers,
  ordered: readonly string[],
): SelectionState {
  if (modifiers.range && current.anchor !== null) {
    const from = ordered.indexOf(current.anchor);
    const to = ordered.indexOf(path);
    if (from !== -1 && to !== -1) {
      const [start, end] = from <= to ? [from, to] : [to, from];
      // 锚点保持不动、动端落到这一次的落点：连着按 Shift 点几次，起点始终是同一个，
      // 而"下一次从哪儿继续"跟着最后一次走。
      return {
        paths: new Set(ordered.slice(start, end + 1)),
        anchor: current.anchor,
        lead: path,
      };
    }
  }
  if (modifiers.toggle) {
    const paths = new Set(current.paths);
    if (!paths.delete(path)) {
      paths.add(path);
    }
    return { paths, anchor: path, lead: path };
  }
  return singleSelection(path);
}

/**
 * 右键落点上的选择：**落在选中集里就保持整批**，落在集外则先单选它。
 *
 * 这是所有文件管理器的共同口径，也是"菜单动作与看到的高亮必须是同一批对象"这条
 * 要求的唯一解——右键无条件清空整批，用户刚选好的十条会在他找菜单的路上消失。
 */
export function selectionForContextMenu(
  current: SelectionState,
  path: string,
): SelectionState {
  return current.paths.has(path) ? current : singleSelection(path);
}

/** 收敛到此刻在场的行；锚点跟着一起收敛（不在场的锚点会让下一次连选落空）。 */
export function prunedSelection(
  current: SelectionState,
  alive: readonly string[],
): SelectionState {
  const present = new Set(alive);
  const kept = [...current.paths].filter((path) => present.has(path));
  const keepEnd = (end: string | null) => (end !== null && present.has(end) ? end : null);
  if (
    kept.length === current.paths.size &&
    keepEnd(current.anchor) === current.anchor &&
    keepEnd(current.lead) === current.lead
  ) {
    return current;
  }
  return { paths: new Set(kept), anchor: keepEnd(current.anchor), lead: keepEnd(current.lead) };
}

/**
 * 按**所见顺序**排好的选中路径——批量命令一律用它提交。
 *
 * 勾选先后不是顺序（PRD-06-047 的既有裁决）：同样几条笔记，先点哪一条都会喂给汇总
 * 一份不同的输入，而用户看不见这个变量。所见即所得，也就没有第二个顺序需要解释。
 */
export function orderedSelection(
  selection: SelectionState,
  ordered: readonly string[],
): string[] {
  return ordered.filter((path) => selection.paths.has(path));
}

/** 方向键要落到的下一行（越界即停在两端，不循环——列表两头是真实的边界）。 */
export function rowAtOffset(
  ordered: readonly string[],
  from: string | null,
  offset: number,
): string | null {
  if (ordered.length === 0) {
    return null;
  }
  if (from === null) {
    return offset > 0 ? ordered[0] : ordered[ordered.length - 1];
  }
  const index = ordered.indexOf(from);
  if (index === -1) {
    return offset > 0 ? ordered[0] : ordered[ordered.length - 1];
  }
  const next = Math.min(Math.max(index + offset, 0), ordered.length - 1);
  return ordered[next];
}
