/**
 * 编辑面的回放入口（PRD-04-020a / PRD-04-064/066）。
 *
 * 同一篇录音笔记此前有**两个能力完全不对等的入口**：从「录音」点开落进
 * `SyncPlaybackView`（整轨进度条、cue 双向联动、说话人名册、双语切换、导出），从**文件树**
 * 点开落进编辑器（一堆裸时间戳文本，时间戳不可点、没有播放器）。而文件树是默认落点
 * （PRD-07-001「默认落在笔记」），于是用户最自然的那条路恰好通向能力最弱的那一面，
 * 且界面上**没有任何线索**说明"换个地方点开就有播放器"。本徽标就是那条线索。
 *
 * **两枚动作，不是一枚**（2026-09-08 随 ADR-118 修订 PRD-04-020a ①）：
 * - 「在此播放」= 把这一篇交给**常驻播放宿主**，用户留在编辑器里听，底部出现「正在播放」条；
 * - 「打开回放」= 原来的深链，去回放视图做校对 / 翻译 / 说话人这类重活。
 *
 * 修订前这里只有后者，理由是"不在编辑器里重建播放器，否则两套 cue 状态、两处 seek、两个
 * 当前行"。那条理由现在**更强地成立**：宿主是全应用唯一的播放器，编辑器只是订阅它——两套
 * 状态从结构上就不可能出现，而"想听一句必须先跳走"这个代价也不必再付。
 *
 * **反方向仍然不给**：PRD-04-020 明写「编辑默认留在录音页内完成，避免用户从回放上下文
 * 跳走」。回放面里已有的行内编辑不该反过来把人弹进编辑器。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { ExpandIcon, PlayIcon } from "../../components/ui/Icon";
import { usePlaybackHostOptional } from "./playback-host";
import { goTo } from "../../lib/navigate";

/** 这一篇属于「录音」还是「转写」空间；`null` = 没有可回放的媒体，不渲染入口。 */
export type PlaybackSection = "recordings" | "transcripts";

/**
 * 探测这一篇是不是可回放的录音或转写。
 *
 * **问 Rust 这一条是不是该类型，不拿路径反推、也不拿"它在不在某个列表里"当判据**
 * （ADR-003）。后者正是处理队列面板注释里记下的那个坑：列表分页之后，"在不在列表里"
 * 只代表**已加载的那几页**，于是第 300 条转写会被静默判成别的东西。
 *
 * **两条查询并发**：它们互不依赖，串行只是白白多一个往返。
 *
 * **`path_prefix` 之后仍要逐条核对 `hit.path === notePath`**：前缀匹配会把
 * `Recordings/会议.md` 的查询命中到 `Recordings/会议.md.bak` 这类同前缀的邻居上。
 */
async function probePlaybackSection(notePath: string): Promise<PlaybackSection | null> {
  const query = (noteType: "recording" | "transcript") =>
    commands.search({
      text: null,
      note_type: noteType,
      tag: null,
      path_prefix: notePath,
      limit: 1,
    });

  const [recording, transcript] = await Promise.all([query("recording"), query("transcript")]);
  const hit = (result: Awaited<ReturnType<typeof query>>) =>
    result.status === "ok" && result.data.some((row) => row.path === notePath);

  if (hit(recording)) {
    return "recordings";
  }
  if (hit(transcript)) {
    return "transcripts";
  }
  // 对话笔记同属来源层（ADR-024）但没有媒体可放，落这里 —— 与普通笔记一样不渲染入口。
  return null;
}

export interface PlaybackEntryBadgeProps {
  notePath: string | null;
  /**
   * 探测结果上报给编辑器。
   *
   * **一次探测两处用**：编辑器要用它决定"这一篇的时间戳行能不能点"，如果编辑器自己再探
   * 一次，同一个问题就有了两次往返和两个可能不一致的答案。
   */
  onSectionChange?: (section: PlaybackSection | null) => void;
}

export function PlaybackEntryBadge({ notePath, onSectionChange }: PlaybackEntryBadgeProps) {
  const { t } = useTranslation(["playback", "workspace"]);
  const [section, setSection] = useState<PlaybackSection | null>(null);
  const host = usePlaybackHostOptional();

  useEffect(() => {
    if (!notePath) {
      setSection(null);
      return;
    }
    let disposed = false;
    // 切到另一篇时先清空：否则上一篇的入口会在新一篇的探测回来之前**继续挂着**，
    // 而它按下去送用户去的是上一篇。
    setSection(null);
    void probePlaybackSection(notePath).then((next) => {
      if (!disposed) {
        setSection(next);
      }
    });
    return () => {
      disposed = true;
    };
  }, [notePath]);

  // 上报与本地状态分开一个 effect：回调的引用变化不该触发重新探测。
  useEffect(() => {
    onSectionChange?.(section);
  }, [section, onSectionChange]);

  if (!section || !notePath) {
    return null;
  }

  const scheme = section === "recordings" ? "recording" : "transcribe";

  return (
    <>
      {/* 主动作：就地听。没有宿主的窗口（副库窗口）里不出现——那里没有播放器可托付。 */}
      {host !== null && (
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-playback-entry"
          title={t("playback:playHereHint")}
          onClick={() => {
            host.open(notePath, { play: true });
          }}
        >
          <PlayIcon size={13} />
          <span>{t("playback:playHere")}</span>
        </button>
      )}
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm luna-playback-entry"
        title={t("playback:openFromEditorHint")}
        onClick={() => {
          // 深链带上对象 → 落点那一格**顺带选中这一篇**（`workspace-routes` 的 `material`
          // 意图）。只切格不选中的话，用户点的是某一篇上的入口、看到的却是一张列表。
          // 路径裸传（与 `notes://` 同口径，见 `workspace-routes` 那条注释）。
          goTo(`${scheme}://${notePath}`);
        }}
      >
        <ExpandIcon size={13} />
        <span>{t("playback:openView")}</span>
      </button>
    </>
  );
}
