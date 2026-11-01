/**
 * 录音 / 转写行的元数据（PRD-07-007）：修改时间 + 处理状态胶囊。
 *
 * 状态来自处理队列的**同一份**模块级订阅，前端只按 `note_rel` 找到这一篇的任务并把 Rust
 * 已判定的状态翻成词——此前只有打开队列面板才知道哪条还在转、哪条失败了。
 *
 * **每行自己按篇订**（2026-10-15 真机卡顿）：此前这段在工作区根里按整份队列现算，于是任务
 * 每推进一拍，根连同整个工作区都要重渲染一遍。现在只有「这一行的状态词变了」才重渲染这一行。
 */

import { useTranslation } from "react-i18next";
import type { SearchHitDto } from "../ipc/bindings";
import { noteTaskState } from "../features/processing/processing-labels";
import { useProcessingQueueSelector } from "../features/processing/use-processing-queue";
import { formatRelativeTime } from "../lib/format-relative-time";

export function ArchiveRowMeta({
  item,
  timeOf,
}: {
  item: SearchHitDto;
  timeOf: (item: SearchHitDto) => number | null;
}) {
  const { t, i18n } = useTranslation("workspace");
  const state = useProcessingQueueSelector((queue) => noteTaskState(queue.items, item.path));
  // 行上的时间与这一档的分组同一个时间（`rowTimeFor`）：按录制时间排就是「它录于何时」
  // （`created_ms`，改一次标题不会把它顶到今天，PRD-07-007），按最近修改排就是修改时间——
  // 否则「今天」分组下的一行会写着「昨天」（真机 2026-10-06）。
  const when = formatRelativeTime(timeOf(item), Date.now(), i18n.language);
  // 「这条还要不要处理」：没有任何一条媒体转写过、且此刻也没在队列里，才挂「未转写」。
  const untranscribed = state === null && !item.transcribed && (item.duration_ms ?? 0) > 0;
  return (
    <>
      {when && <span>{when}</span>}
      {state && (
        <span className="luna-archive-status" data-state={state}>
          {t(`archive.status.${state}`)}
        </span>
      )}
      {untranscribed && (
        <span className="luna-archive-status" data-state="untranscribed">
          {t("archive.status.untranscribed")}
        </span>
      )}
    </>
  );
}
