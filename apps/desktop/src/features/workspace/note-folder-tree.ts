/**
 * 功能视图列表（速记 / 录音 / 转写 / AI 对话）的**文件夹分组**（PRD-07-062，纯函数）。
 *
 * 这四个目的地的清单语义是「该类型的**全部**笔记」（按索引 `type` 查询），条目会随使用
 * 无限累积而只能靠搜索找；用户要的是把它们分门别类收进文件夹。
 *
 * **分组是呈现，不是新的数据模型**：笔记的归属**就是它在 Vault 里的路径**（笔记即文件），
 * 因此本模块只把已有的 `path` 折成一棵展示用的树，不引入任何"集合/分类"这类第二事实源。
 * 与 `tag-tree.ts` 由扁平标签折出嵌套树是同一心智，故同样放在前端纯逻辑层。
 *
 * **根目录相对化**：录音笔记天然都落在 `vault.recordings_dir` 之下，若按 Vault 根渲染，
 * 整棵树只有一个"录音"节点、每次都要多展开一层。故按该类型的归档目录裁掉前缀；
 * 不在该目录下的笔记（用户手动挪走过）**不隐藏也不假装**，按其真实路径分组。
 */

/** 参与分组的最小条目形状（`SearchHitDto` 结构子集，便于无头测试）。 */
export interface FolderedItem {
  path: string;
}

export interface FolderNode<T extends FolderedItem> {
  /** Vault 相对路径（文件夹命令直接用它，不必再拼）。 */
  path: string;
  /** 该层显示名。 */
  name: string;
  /** 子文件夹（按名称升序）。 */
  folders: FolderNode<T>[];
  /** 直属条目（不含子文件夹里的）。 */
  items: T[];
  /** 含全部子孙的条目总数——折叠时要显示的就是这个数。 */
  total: number;
}

export interface FolderedList<T extends FolderedItem> {
  /** 根层直属条目（不在任何子文件夹里）。 */
  items: T[];
  folders: FolderNode<T>[];
}

/** 去掉首尾斜杠，把 `""` / `"/"` 归一为 `""`。 */
function normalizeDir(dir: string): string {
  return dir.replace(/^\/+|\/+$/gu, "");
}

/**
 * 条目的目录段 + 这些段所挂的 Vault 相对前缀。
 *
 * 在根目录下 → 段是相对根的、前缀是根；不在根下 → 段已经是相对 Vault 根的、前缀为空。
 * **两者必须一起返回**：显示名用段，而文件夹命令要的是完整的 Vault 相对路径，
 * 若一律拼上根前缀，挪到别处的笔记会被算进一个并不存在的目录。
 */
function segmentsOf(path: string, root: string): { segments: string[]; base: string } {
  const { segments, base } = dirSegmentsOf(path, root);
  segments.pop(); // 去掉文件名本身
  return { segments, base };
}

/**
 * 同上，但用于**目录路径**：最后一段是文件夹名本身，不能砍掉。
 *
 * 这就是它必须与 `segmentsOf` 分开的全部原因。第一版把目录也喂给 `segmentsOf`，
 * 于是 `Recordings/空文件夹` 被当成"根目录下的一个文件"，段列表为空、节点建不出来——
 * 现象与修之前一模一样（空目录仍然看不见），而代码看上去已经处理了。
 */
function dirSegmentsOf(path: string, root: string): { segments: string[]; base: string } {
  const prefix = root === "" ? "" : `${root}/`;
  const inRoot = prefix !== "" && path.startsWith(prefix);
  const rel = inRoot ? path.slice(prefix.length) : path;
  return {
    segments: rel.split("/").filter((part) => part !== ""),
    base: inRoot ? root : "",
  };
}

/**
 * 把扁平清单折成文件夹树。
 *
 * @param items 该类型的全部笔记（顺序即调用方已排好的顺序，本函数**不重排条目**——
 *              列表的排序方式是用户可选的（PRD-07-007），分组不该悄悄改掉它）。
 * @param rootDir 该类型的归档目录（Vault 相对；`""` = 按 Vault 根分组）。
 */
export function groupByFolder<T extends FolderedItem>(
  items: readonly T[],
  rootDir: string,
  dirs: readonly string[] = [],
): FolderedList<T> {
  const root = normalizeDir(rootDir);
  const result: FolderedList<T> = { items: [], folders: [] };
  /** 路径 → 节点，避免每层线性查找。 */
  const index = new Map<string, FolderNode<T>>();

  /** 建出（或取到）某条路径上的全部层级节点，返回最深那一个。 */
  const ensure = (segments: readonly string[], base: string): FolderNode<T> | null => {
    let siblings = result.folders;
    // 索引键即完整 Vault 相对路径：`录音/A` 与 Vault 根下的 `A` 是两个不同的文件夹。
    let key = base;
    let node: FolderNode<T> | null = null;
    for (const segment of segments) {
      key = key === "" ? segment : `${key}/${segment}`;
      let found = index.get(key);
      if (found === undefined) {
        found = { path: key, name: segment, folders: [], items: [], total: 0 };
        index.set(key, found);
        siblings.push(found);
      }
      node = found;
      siblings = found.folders;
    }
    return node;
  };

  // **先把真实目录建成节点**（Rust 的 `vault_list_dirs` 给的磁盘事实）：只靠条目路径
  // 折树的话，没有条目的目录压根不生成节点——用户刚新建的那个因此永远看不见，而「新建
  // 文件夹」按钮就摆在清单顶上（真机 2026-09-08 上报）。空目录 `total` 保持 0。
  for (const dir of dirs) {
    const { segments, base } = dirSegmentsOf(dir, root);
    if (segments.length > 0) ensure(segments, base);
  }

  for (const item of items) {
    const { segments, base } = segmentsOf(item.path, root);
    if (segments.length === 0) {
      result.items.push(item);
      continue;
    }
    ensure(segments, base);
    // 计数逐层累加（`ensure` 只保证节点在，不碰计数）。
    let key = base;
    for (const segment of segments) {
      key = key === "" ? segment : `${key}/${segment}`;
      const node = index.get(key);
      if (node !== undefined) node.total += 1;
    }
    index.get(key)?.items.push(item);
  }

  sortFolders(result.folders);
  return result;
}

/** 文件夹按名称升序（条目顺序不动，见 `groupByFolder` 的说明）。 */
function sortFolders<T extends FolderedItem>(folders: FolderNode<T>[]): void {
  folders.sort((a, b) => a.name.localeCompare(b.name));
  for (const folder of folders) {
    sortFolders(folder.folders);
  }
}

/**
 * 「移动到 ▸」子菜单的候选目录（根目录 + 全部子文件夹，深度优先、路径升序）。
 *
 * 候选来自**这棵树上的节点**，故不会出现"移到一个不存在的地方"。
 *
 * **空文件夹自 2026-09-08 起也是候选**：树不再只从 `items` 的路径折出来，`groupByFolder`
 * 还会把 Rust 列出的真实目录（`vault_list_dirs`）建成节点。此前没有目录源，空目录不生成
 * 节点，于是"先新建文件夹再把东西移进去"这条最自然的路走不通——而「新建文件夹」按钮就在
 * 清单顶上，用户点了、磁盘上也真建了，界面却当它不存在。
 */
export function folderChoices<T extends FolderedItem>(
  tree: FolderedList<T>,
  rootDir: string,
): { path: string; depth: number; name: string }[] {
  const out: { path: string; depth: number; name: string }[] = [];
  const walk = (folders: readonly FolderNode<T>[], depth: number) => {
    for (const folder of folders) {
      out.push({ path: folder.path, depth, name: folder.name });
      walk(folder.folders, depth + 1);
    }
  };
  walk(tree.folders, 1);
  return [{ path: normalizeDir(rootDir), depth: 0, name: "" }, ...out];
}

/**
 * 这些对象是不是**已经全在**该目录里（「移动到 ▸」据此置灰那一项）。
 *
 * **只判"要不要给这个动作"，不算落点路径**：落点由 Rust 拼（`vault_move_into`，
 * ADR-003）。此前这里返回的是移动后的完整路径，于是"目标目录 + 末级名"这段拼接在前端
 * 有了第二份实现——而真正落盘的是 Rust 那份。
 */
/**
 * 这份清单是不是**真的**没东西可列（该渲染空态而不是树）。
 *
 * 只看条目数是错的（真机 2026-09-08「速记、AI 会话都无法创建文件夹」）：零条目时空态分支
 * 先于树，于是点「新建文件夹」什么都不出现（新建输入行长在树里），建好的空目录也永远看不见
 * ——磁盘上 `Chats/abc` 明明已经在了。有目录、或正在输入新建行，就得渲染树。
 * 筛选中（有查询）例外：树被平铺，空目录不显示，零命中就该老实说"没有匹配"。
 */
export function listHasNothing(input: {
  itemCount: number;
  dirCount: number;
  creating: boolean;
  filtering?: boolean;
}): boolean {
  if (input.itemCount > 0 || input.creating) return false;
  return input.filtering === true || input.dirCount === 0;
}

/**
 * 这一批拖到 `targetDir` 上能不能放：至少有一条**不在**那个目录里。
 *
 * 松手与高亮同一判据：全都已经在里面时高亮不亮、松手也不发命令——否则用户看着一个
 * "放不进去"的落点却真的把命令发出去了（整批仍由 Rust 权威复验，这里只管画不画高亮）。
 */
export function canDropInto(paths: readonly string[], targetDir: string): boolean {
  return paths.length > 0 && !allInDir(paths, targetDir);
}

export function allInDir(paths: readonly string[], targetDir: string): boolean {
  const dir = normalizeDir(targetDir);
  return paths.every((path) => {
    const base = path.split("/").pop() ?? path;
    return (dir === "" ? base : `${dir}/${base}`) === path;
  });
}
