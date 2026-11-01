/**
 * 标签浏览器控制器（PRD-06-033）：主侧栏标签树与主区笔记清单共享唯一实例。
 *
 * Rust 下发层级摘要与去重计数；这里仅编排加载、选择和刷新。选中标签时立即清掉上一标签的
 * 结果，避免新标题下短暂展示旧笔记；请求序号保证快速切换时迟到响应不会覆盖当前选择。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  commands,
  type SearchHitDto,
  type TagSummaryDto,
  type TagTopicActionsDto,
} from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";

const TAG_NOTE_LIMIT = 100;

export type TagLoadState = "idle" | "loading" | "ready" | "error";

export interface TagBrowserController {
  summaries: TagSummaryDto[];
  /**
   * 这个库此刻生成 AI 关系标签吗（`vault.graph_ai_tags`，随树一次下发）。
   * 关着时「AI 标签」分组**整组不出现**（PRD-07-024）——前端不去读设置键。
   */
  aiTagsEnabled: boolean;
  /**
   * 选中标签此刻的动作资格（PRD-06-033 的首选/次选分流）。判定全在 Rust；
   * `null` = 还没取到（未选标签或正在取）。
   */
  topicActions: TagTopicActionsDto | null;
  selectedTag: string | null;
  selectedSummary: TagSummaryDto | null;
  notes: SearchHitDto[];
  summaryState: TagLoadState;
  noteState: TagLoadState;
  truncated: boolean;
  selectTag: (tag: string) => void;
  refresh: () => void;
}

/** 刷新后保留仍存在的选择；首次进入或标签消失时选后端排序的第一项。 */
export function resolveTagSelection(
  summaries: readonly TagSummaryDto[],
  selectedTag: string | null,
): string | null {
  if (selectedTag) {
    const retained = summaries.find(
      (summary) => summary.tag.toLowerCase() === selectedTag.toLowerCase(),
    );
    if (retained) return retained.tag;
  }
  return summaries[0]?.tag ?? null;
}

export function useTagBrowser(enabled: boolean): TagBrowserController {
  const [summaries, setSummaries] = useState<TagSummaryDto[]>([]);
  const [aiTagsEnabled, setAiTagsEnabled] = useState(true);
  const [topicActions, setTopicActions] = useState<TagTopicActionsDto | null>(null);
  const [selectedTag, setSelectedTag] = useState<string | null>(null);
  const [notes, setNotes] = useState<SearchHitDto[]>([]);
  const [summaryState, setSummaryState] = useState<TagLoadState>("idle");
  const [noteState, setNoteState] = useState<TagLoadState>("idle");
  const selectedTagRef = useRef<string | null>(null);
  const summaryRequestRef = useRef(0);
  const noteRequestRef = useRef(0);

  const loadNotes = useCallback((tag: string) => {
    const request = ++noteRequestRef.current;
    selectedTagRef.current = tag;
    setSelectedTag(tag);
    setNotes([]);
    setNoteState("loading");
    // 动作资格与笔记结果同一拍取：分两拍会让按钮先摆出上一条标签的资格再跳一次。
    setTopicActions(null);
    void commands.tagTopicActions(tag).then((result) => {
      if (request !== noteRequestRef.current) return;
      setTopicActions(result.status === "ok" ? result.data : null);
    });
    void commands
      .search({
        text: null,
        note_type: null,
        tag,
        path_prefix: null,
        limit: TAG_NOTE_LIMIT,
      })
      .then((result) => {
        if (request !== noteRequestRef.current) return;
        if (result.status === "ok") {
          setNotes(result.data);
          setNoteState("ready");
        } else {
          setNotes([]);
          setNoteState("error");
        }
      });
  }, []);

  const refresh = useCallback(() => {
    const request = ++summaryRequestRef.current;
    setSummaryState("loading");
    void commands.tagSummaries().then((result) => {
      if (request !== summaryRequestRef.current) return;
      if (result.status === "error") {
        setSummaries([]);
        setSummaryState("error");
        setTopicActions(null);
        selectedTagRef.current = null;
        setSelectedTag(null);
        setNotes([]);
        setNoteState("idle");
        return;
      }

      const nextSelection = resolveTagSelection(result.data.tags, selectedTagRef.current);
      setSummaries(result.data.tags);
      setAiTagsEnabled(result.data.ai_tags_enabled);
      setSummaryState("ready");
      if (nextSelection) {
        loadNotes(nextSelection);
      } else {
        ++noteRequestRef.current;
        selectedTagRef.current = null;
        setSelectedTag(null);
        setNotes([]);
        setNoteState("idle");
        setTopicActions(null);
      }
    });
  }, [loadNotes]);

  useEffect(() => {
    if (!enabled) return;
    refresh();
  }, [enabled, refresh]);

  useVaultReload(refresh, { enabled });

  const selectedSummary =
    summaries.find((summary) => summary.tag === selectedTag) ?? null;

  return {
    summaries,
    aiTagsEnabled,
    topicActions,
    selectedTag,
    selectedSummary,
    notes,
    summaryState,
    noteState,
    truncated:
      noteState === "ready" && selectedSummary !== null && selectedSummary.note_count > notes.length,
    selectTag: loadNotes,
    refresh,
  };
}
