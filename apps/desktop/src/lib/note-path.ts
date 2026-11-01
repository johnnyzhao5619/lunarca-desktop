// 笔记路径的纯展示计算（跨 feature 共用，故放 lib/ 而不是任一 feature 内——
// 与 `shortcut-display.ts` 同一先例：多处需要的纯函数上提，不各写一份）。

/** 路径的文件名段（无目录时即整段）。`noteStem` 与回收站排序、附件视图共用。 */
export function fileNameOf(path: string): string {
  return path.split("/").pop() ?? path;
}

/** 笔记的展示名：取文件名并去掉 `.md` 后缀（目录名与非 md 文件原样返回）。 */
export function noteStem(path: string): string {
  const name = fileNameOf(path);
  return name.endsWith(".md") ? name.slice(0, -3) : name;
}

/** 笔记路径的展示形式：保留目录层级、只去掉 `.md` 后缀（链接面板列路径用）。 */
export function notePathLabel(path: string): string {
  return path.endsWith(".md") ? path.slice(0, -3) : path;
}

/** `notes://` 深链解析结果（PRD-16-012：出处跳转带行锚）。 */
export interface NoteRouteTarget {
  /** 笔记相对路径。 */
  path: string;
  /** 目标行（1-based）；`null` = 只打开笔记不定位。 */
  line: number | null;
}

/**
 * 解析 `notes://<path>[#L<n>]` 深链。
 *
 * 行锚用 web 查看器惯用的 `#L12`（GitHub 同形）而非编辑器惯用的 `:12`——路径里出现
 * 冒号是合法的，`#` 则天然是 URL 片段分隔符，不会与文件名争。锚只是**路由参数**：
 * 引用记录里的权威锚是复合锚（标题路径 + 引文 ± 上下文），行号由 Rust 解析后才产生
 * （ADR-026 ⑥），故这里拿到的行号已经是"此刻文件里的真行号"。
 */
export function parseNoteRoute(route: string): NoteRouteTarget | null {
  const prefix = "notes://";
  if (!route.startsWith(prefix)) {
    return null;
  }
  const rest = route.slice(prefix.length);
  if (!rest) {
    return null;
  }
  const hash = rest.lastIndexOf("#L");
  if (hash < 0) {
    return { path: rest, line: null };
  }
  const line = Number(rest.slice(hash + 2));
  if (!Number.isInteger(line) || line < 1) {
    // 不是合法行锚就当路径的一部分——静默丢掉一段路径比不定位更糟。
    return { path: rest, line: null };
  }
  return { path: rest.slice(0, hash), line };
}
