// wiki-link 语法（PRD-06-030）：编辑面装饰与点击、实时预览括号折叠、阅读视图共用这一份。

/** wiki-link 语法单一来源。全局正则：每次扫描前把 `lastIndex` 归零。 */
export const WIKI_LINK_RE = /\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g;
