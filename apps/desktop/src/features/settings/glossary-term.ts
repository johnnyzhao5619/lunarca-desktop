/**
 * 「选中的这段文字能不能当作一条词条」的判据（PRD-07-061，纯函数）。
 *
 * 判据集中在一处，是因为「添加到个人词典」要出现在**多个**内容面（笔记正文、回放字幕），
 * 而各处若自己判一遍，很快就会出现"编辑器里选一段话能加、字幕里不能"这类说不清的差异。
 */

/**
 * 词条长度上限（字符）。
 *
 * 词典的用途是让 ASR 认对**术语、人名、缩写**，落地时会拼进 whisper 的 initial prompt
 * （预算约 224 token，见 `ASR_PROMPT_BUDGET_CHARS`）。选中一整段话加进去，既不会提升
 * 识别率，还会把预算挤占掉、把真正的术语挤出提示——所以这不是"防呆的宽松上限"，
 * 而是这条功能本身的适用边界。
 */
export const GLOSSARY_TERM_MAX_CHARS = 32;

/**
 * 把一段选区文本收敛为可用的词条，不适用时返回 `null`（菜单据此不渲染该项）。
 *
 * 返回 `null` 而不是"渲染一个禁用项"：一条永远点不动的菜单项只是噪声，
 * 而用户此刻并没有在找这个功能。
 */
export function glossaryTermFromSelection(raw: string | null | undefined): string | null {
  if (raw === null || raw === undefined) {
    return null;
  }
  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    return null;
  }
  // 跨行的选区一定不是词条（是段落、是整条字幕）。
  if (/[\r\n]/.test(trimmed)) {
    return null;
  }
  if (trimmed.length > GLOSSARY_TERM_MAX_CHARS) {
    return null;
  }
  return trimmed;
}

/** 取当前文档选区并按上面的判据收敛（DOM 侧唯一取数点）。 */
export function glossaryTermFromDocumentSelection(): string | null {
  return glossaryTermFromSelection(window.getSelection()?.toString() ?? null);
}
