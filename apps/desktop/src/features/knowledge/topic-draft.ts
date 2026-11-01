/**
 * 主题页初稿的发起（PRD-16-016/017，ADR-134 证据格式）。
 *
 * 新建主题页走「先预览、再确认」（PRD-16-045/046）：发起只是把请求交给已挂载的
 * `TopicDraftHost`，由它调用 Rust `prepare_topic_draft` 准备并封存初稿、逐条展示、
 * 用户确认后 `confirm_topic_draft` 发布同一份字节。候选来源由 Rust 按标签取，前端不传笔记清单。
 */

import { commands } from "../../ipc/bindings";

/** 请求为这个标签新建主题页（由 `TopicDraftHost` 承接：准备 → 预览 → 确认）。 */
export const TOPIC_DRAFT_REQUEST_EVENT = "luna:topic-draft-request";

export interface TopicDraftRequestDetail {
  tag: string;
}

export function requestTopicDraft(tag: string): void {
  window.dispatchEvent(
    new CustomEvent<TopicDraftRequestDetail>(TOPIC_DRAFT_REQUEST_EVENT, { detail: { tag } }),
  );
}

/** 忽略一条主题页建议（✕ 即对该标签永久安静，PRD-16-004）。 */
export async function ignoreTopicSuggestion(tag: string, suggestionToken: string): Promise<void> {
  await commands.ignoreTopicSuggestion(tag, suggestionToken);
}
