/**
 * 「只改了元数据」的外部写入（ADR-138 ① 保存护栏的一个窄口子）。
 *
 * Lunarca 自己在后台写回的 frontmatter——AI 关系标签（`luna.ai_tags`）、项目阶段
 * （`luna.project_stage`）之类——会以 `note://written` 广播到正打开这一篇的编辑器。用户这时
 * 若正在打字（缓冲区未保存），此前一律按「文件已在别处修改」处理：暂停自动保存、弹横幅让人选。
 * 可正文一个字都没动，用户没有任何需要选的东西（真机 2026-10-17：写脚本时每次自动保存后
 * 几秒，后台打完标签就弹一次）。
 *
 * 判据只有一条：对方的**正文**与上次保存的正文逐字相同。满足时由编辑器用 Rust 三路合并
 * （`note_merge`）把对方的 frontmatter 并进缓冲区；合并不干净（用户自己也改了同一处元数据）
 * 或判据不成立，照旧走横幅——这里不替用户裁决任何正文冲突。
 */

import { splitFrontmatter } from "./reading-view";

/** `incoming` 相对 `saved` 是不是只改了 frontmatter（正文逐字相同、且两者确有不同）。 */
export function isMetadataOnlyWrite(saved: string, incoming: string): boolean {
  if (saved === incoming) {
    return false;
  }
  return splitFrontmatter(saved).body === splitFrontmatter(incoming).body;
}
