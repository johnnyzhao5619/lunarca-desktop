// 格式按钮的图标映射（格式工具栏与选区浮动格式条共用，DRY）：条目 key 来自
// `@lunarca/editor-core/editor-actions` 的构建器，这里只答"哪个 key 画哪枚图标"。
import {
  BoldIcon,
  CodeIcon,
  ItalicIcon,
  ListIcon,
  ListOrderedIcon,
  ListTodoIcon,
  QuoteIcon,
  StrikethroughIcon,
} from "../ui/Icon";

export const INLINE_ICONS: Record<string, typeof BoldIcon> = {
  bold: BoldIcon,
  italic: ItalicIcon,
  strike: StrikethroughIcon,
  code: CodeIcon,
};

export const BLOCK_ICONS: Record<string, typeof BoldIcon> = {
  quote: QuoteIcon,
  bullet: ListIcon,
  ordered: ListOrderedIcon,
  task: ListTodoIcon,
};
