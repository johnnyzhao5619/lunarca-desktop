/**
 * 创作看板的「日历」视图（PRD-19-040）：按计划发布日期把项目排进月历，旁边一栏是还没排期的。
 *
 * 把卡片拖到某一天 = 把那一天写进项目主页的 `luna.project_publish_date`；拖回「未排期」= 清掉。
 * 拖拽走全应用共用的指针拖拽基元；不便拖拽时，项目页头的日期框做同一件事。哪一天是计划发布
 * 日期由 Rust 判定下发，月历几何（一个月排几行、每格是哪天）是这里的纯函数（`creator-calendar`）。
 */

import { useRef, useState, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { useTranslation } from "react-i18next";

import type { ProjectCardDto } from "../../ipc/bindings";
import { ChevronLeftIcon, ChevronRightIcon } from "../../components/ui/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { cssVariables } from "../../lib/css-variables";
import { formatMonthTitle, weekdayNames } from "../../lib/format-datetime";
import { DRAG_GHOST_OFFSET_PX, usePointerDrag } from "../../lib/pointer-drag";
import {
  UNSCHEDULED_DROP_ID,
  dateDropId,
  dateOfDropId,
  isoDay,
  monthOf,
  monthWeeks,
  shiftMonth,
  weekStartsOn,
  type CalendarMonth,
} from "./creator-calendar";

/** 一格里最多直接列几张（其余折成「还有 N 个」，点开项目在列表里找）。 */
const CARDS_PER_DAY = 3;

export interface CreatorCalendarProps {
  projects: readonly ProjectCardDto[];
  /** 正在写盘的那一张。 */
  moving: string | null;
  onOpen: (card: ProjectCardDto) => void;
  onCardMenu: (event: ReactMouseEvent, card: ProjectCardDto) => void;
  /** 改计划发布日期（`null` = 清掉）。 */
  onSetDate: (card: ProjectCardDto, date: string | null) => void;
}

export function CreatorCalendar({ projects, moving, onOpen, onCardMenu, onSetDate }: CreatorCalendarProps) {
  const { t, i18n } = useTranslation("workspace");
  const today = isoDay(new Date());
  const [month, setMonth] = useState<CalendarMonth>(() => monthOf(new Date()));
  const weekStart = weekStartsOn(i18n.language);
  const weeks = monthWeeks(month, weekStart, today);

  /** 刚拖完的那一张：松手紧跟着的那次 click 不是「打开」。 */
  const justDragged = useRef<string | null>(null);
  const endDrag = (card: ProjectCardDto) => {
    justDragged.current = card.path;
    window.setTimeout(() => {
      justDragged.current = null;
    }, 0);
  };
  const drag = usePointerDrag<ProjectCardDto>(
    (card, dropId) => {
      endDrag(card);
      const date = dateOfDropId(dropId);
      if (date !== undefined && date !== card.publish_date) onSetDate(card, date);
    },
    (card) => endDrag(card),
  );

  const byDate = new Map<string, ProjectCardDto[]>();
  const unscheduled: ProjectCardDto[] = [];
  for (const card of projects) {
    if (card.publish_date) {
      byDate.set(card.publish_date, [...(byDate.get(card.publish_date) ?? []), card]);
    } else {
      unscheduled.push(card);
    }
  }

  const chip = (card: ProjectCardDto) => (
    <button
      key={card.path}
      type="button"
      className="luna-creator-calendar__chip"
      title={card.path}
      data-dragging={drag.payload?.path === card.path || undefined}
      disabled={moving === card.path}
      onPointerDown={(event: ReactPointerEvent) => {
        if (event.button === 0) drag.begin(event, card);
      }}
      onClick={() => {
        if (justDragged.current !== card.path) onOpen(card);
      }}
      onContextMenu={(event) => onCardMenu(event, card)}
    >
      {card.title}
    </button>
  );

  return (
    <div className="luna-creator-calendar" data-dragging={drag.payload !== null || undefined}>
      <section className="luna-creator-calendar__month" aria-label={t("creator.calendar.label")}>
        <header className="luna-creator-calendar__nav">
          <IconButton
            density="sm"
            icon={<ChevronLeftIcon size={14} />}
            label={t("creator.calendar.prev")}
            onClick={() => setMonth((current) => shiftMonth(current, -1))}
          />
          <h2 className="luna-creator-calendar__title">{formatMonthTitle(month.year, month.month, i18n.language)}</h2>
          <IconButton
            density="sm"
            icon={<ChevronRightIcon size={14} />}
            label={t("creator.calendar.next")}
            onClick={() => setMonth((current) => shiftMonth(current, 1))}
          />
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => setMonth(monthOf(new Date()))}
          >
            {t("creator.calendar.today")}
          </button>
        </header>
        <div className="luna-creator-calendar__grid" role="grid" aria-label={formatMonthTitle(month.year, month.month, i18n.language)}>
          <div className="luna-creator-calendar__week" role="row">
            {weekdayNames(i18n.language, weekStart).map((name) => (
              <span key={name} className="luna-creator-calendar__weekday" role="columnheader">
                {name}
              </span>
            ))}
          </div>
          {weeks.map((week) => (
            <div key={week[0].iso} className="luna-creator-calendar__week" role="row">
              {week.map((day) => {
                const cards = byDate.get(day.iso) ?? [];
                const dropId = dateDropId(day.iso);
                return (
                  <div
                    key={day.iso}
                    role="gridcell"
                    className="luna-creator-calendar__day"
                    data-drop-id={dropId}
                    data-drop-target={drag.dropId === dropId || undefined}
                    data-outside={!day.inMonth || undefined}
                    data-today={day.today || undefined}
                    aria-label={day.iso}
                  >
                    <span className="luna-creator-calendar__date">{day.day}</span>
                    {cards.slice(0, CARDS_PER_DAY).map(chip)}
                    {cards.length > CARDS_PER_DAY ? (
                      <span className="luna-creator-calendar__more">
                        {t("creator.calendar.more", { count: cards.length - CARDS_PER_DAY })}
                      </span>
                    ) : null}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      </section>
      <section
        className="luna-creator-calendar__unscheduled"
        aria-labelledby="creator-unscheduled-title"
        data-drop-id={UNSCHEDULED_DROP_ID}
        data-drop-target={drag.dropId === UNSCHEDULED_DROP_ID || undefined}
      >
        <h2 id="creator-unscheduled-title">{t("creator.calendar.unscheduled")}</h2>
        <p className="luna-creator-page__hint">{t("creator.calendar.unscheduledHint")}</p>
        {unscheduled.length === 0 ? (
          <p className="luna-creator-page__hint">{t("creator.calendar.noneUnscheduled")}</p>
        ) : (
          <div className="luna-creator-calendar__list">{unscheduled.map(chip)}</div>
        )}
      </section>
      {drag.payload ? (
        <div
          className="luna-drag-ghost"
          style={cssVariables({
            "--drag-x": `${drag.position.x + DRAG_GHOST_OFFSET_PX}px`,
            "--drag-y": `${drag.position.y + DRAG_GHOST_OFFSET_PX}px`,
          })}
        >
          {drag.payload.title}
        </div>
      ) : null}
    </div>
  );
}
