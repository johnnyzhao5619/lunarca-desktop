/**
 * 快捷对话面板入口（`quick-chat` 非激活 NSPanel 窗口）：拥有 `panel` scope 的会话控制器，
 * 内含头部弹层历史（与主窗口工作区的「主侧栏历史」是两个独立 scope，PRD-03-012）。
 */

import { ChatView } from "./ChatView";
import { useChatController } from "./use-chat-controller";

export function ChatPanel() {
  const controller = useChatController("panel");
  return <ChatView controller={controller} panel />;
}
