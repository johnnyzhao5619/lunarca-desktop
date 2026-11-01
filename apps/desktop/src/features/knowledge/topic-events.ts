/**
 * 知识库分区内部的 window 事件（同一 feature 内，不进 `lib/app-events`）。
 *
 * 忽略 / 否决只写随库决定、不改任何文件，`vault://changed` 不会来；而目录的「候选 ≥N」
 * 正依赖这些决定。面板处置成功后发它，目录据此重取一次，不另立轮询。
 */
export const TOPIC_DIRECTORY_STALE_EVENT = "luna:topic-directory-stale";

export function announceTopicDirectoryStale(): void {
  window.dispatchEvent(new Event(TOPIC_DIRECTORY_STALE_EVENT));
}
