/**
 * 搜索语法的稳定呈现顺序。只登记解析器已经支持的能力 id；可见样例与说明均由 i18n 提供。
 */
export const SEARCH_SYNTAX_HINT_IDS = [
  "term",
  "phrase",
  "exclude",
  "tag",
  "path",
  "file",
  "type",
] as const;
