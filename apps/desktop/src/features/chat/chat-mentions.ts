/**
 * `@` 提及的文本层判定（PRD-16-009）。纯函数，可 headless 测试。
 *
 * 这里**只回答两个字符串问题**——「光标此刻是不是正在写一个 `@` 提及」与「选中之后
 * 输入框该变成什么」。「哪些东西可以被提及」「某篇算不算在范围内」都在 Rust
 * （`mention_candidates` / `luna_vault::in_scope`）：那是同一个问题的两半，分家必然漂移。
 */

/** 光标处正在书写的 `@` 提及。 */
export interface ActiveMention {
  /** `@` 本身在文本里的下标。 */
  start: number;
  /** 提及片段的结束下标（= 光标位置）。 */
  end: number;
  /** `@` 之后已经打出来的字（可为空——刚敲下 `@` 时就该弹出候选）。 */
  query: string;
}

/**
 * 光标前是不是一个未闭合的 `@` 提及。
 *
 * 两条判据：
 * - `@` 与光标之间不含空白（提及是一个词；打了空格说明用户已经在写别的了）；
 * - `@` 前**只挡 email 局部串字符**（ASCII 字母数字与 `._%+-`），不是"任何非空白"。
 *   要挡的误触只有 `a@b.com` 这一种形态，而 email 的 local part 只能是 ASCII——
 *   「关于@工作」「（@项目X」这类 CJK/标点前导是**中文输入的常态**（不打空格），
 *   按"非空白就挡"会让 `@` 在中文句子中间彻底失灵（验收轮缺陷）。
 * 空白之外不再排除 query 里的任何字符——「@四月纪要（终版）」是真实名字。
 */
export function activeMention(text: string, caret: number): ActiveMention | null {
  const end = Math.max(0, Math.min(caret, text.length));
  for (let i = end - 1; i >= 0; i -= 1) {
    const ch = text[i]!;
    if (/\s/.test(ch)) {
      return null;
    }
    if (ch === "@") {
      const before = i === 0 ? "" : text[i - 1]!;
      if (before !== "" && /[A-Za-z0-9._%+-]/.test(before)) {
        return null;
      }
      return { start: i, end, query: text.slice(i + 1, end) };
    }
  }
  return null;
}

/**
 * 选中一个候选之后的输入框内容与光标位置。
 *
 * **把 `@…` 片段整个删掉**，不替换成标签名：范围已经变成输入框上方的一枚芯片，
 * 正文里再留一份名字，用户会以为那串文字本身有语义（改掉它会不会换范围？），
 * 而它其实只是残留的字符。删掉之后提问就是一句干净的话。
 */
export function applyMention(
  text: string,
  mention: ActiveMention,
): { text: string; caret: number } {
  const head = text.slice(0, mention.start).replace(/[ \t]+$/, "");
  const tail = text.slice(mention.end);
  // 头尾都非空时补一个空格，免得把前后两个词粘在一起。
  const joiner = head.length > 0 && tail.length > 0 && !/^\s/.test(tail) ? " " : "";
  return { text: `${head}${joiner}${tail}`, caret: head.length + joiner.length };
}
