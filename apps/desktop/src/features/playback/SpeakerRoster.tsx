/**
 * 说话人名册条（PRD-04-056）：回放列表上方的一行说话人 chip（配色点 + 显示名 + 段数）。
 *
 * 两个作用：① **图例**——把 cue 徽标的颜色与「谁」对应起来，扫一眼就知道这场有几个人、
 * 各说了多少；② **全局改名入口**——此前改名只能从某条 cue 的徽标进入（得先找到那个人
 * 说的话），名册条让「给这场的人起名字」成为一件独立、可一次做完的事。
 *
 * 纯呈现：改名输入与落盘由回放视图承载（ADR-003）。
 */

import { useTranslation } from "react-i18next";
import type { SpeakerStatDto } from "../../ipc/bindings";
import { speakerSlot } from "./speaker-view";

export interface SpeakerRosterProps {
  roster: SpeakerStatDto[];
  /** speaker id → 显示名。 */
  displayName: (id: string) => string;
  /** 点击 chip：进入该说话人的改名。 */
  onRename: (speaker: string) => void;
}

export function SpeakerRoster({ roster, displayName, onRename }: SpeakerRosterProps) {
  const { t } = useTranslation("playback");
  if (roster.length === 0) return null;
  return (
    <div className="luna-playback__speakers" role="group" aria-label={t("speaker.rosterLabel")}>
      <span className="luna-playback__speakers-label">{t("speaker.rosterLabel")}</span>
      {roster.map((stat) => (
        <button
          key={stat.id}
          type="button"
          className="luna-speaker-chip"
          data-speaker-slot={speakerSlot(stat.id, roster)}
          title={t("speaker.renameHint")}
          onClick={() => onRename(stat.id)}
        >
          <span className="luna-speaker-chip__dot" aria-hidden="true" />
          {displayName(stat.id)}
          <small>{t("speaker.segmentCount", { count: stat.segment_count })}</small>
        </button>
      ))}
    </div>
  );
}
