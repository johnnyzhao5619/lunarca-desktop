/**
 * 创作看板的三种视图（PRD-19-003）：同一份项目数据、同一套动作，只是看法不同——
 * 看板（按阶段分列拖拽）、列表（按阶段分组的紧凑清单，组可折叠）、表格（一行一个项目，
 * 就地改阶段）；日历见 `CreatorCalendar`。卡片信号（计划发布日期、平台、脚本时长、引文数、发布
 * 进度、下一步，PRD-19-040）由 Rust 一次给齐（`ProjectCardDto.signals`），这里只排版。这是 Notion / Linear / Trello / GitHub Projects 的共同做法：视图切换不改数据，
 * 也不各带一套操作。
 *
 * 阶段、分列、借住（隐藏列里的项目放进哪一列）全部由 Rust 判定下发；这里只渲染，动作一律
 * 交回宿主 `CreatorBoard`（打开、右键菜单、移到某阶段）。
 */

import { useState, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { ChannelDto, ProjectCardDto, ProjectStageDto } from "../../ipc/bindings";
import { ChevronDownIcon, ChevronRightIcon } from "../../components/ui/Icon";
import { SelectBox } from "../../components/ui/SelectBox";
import { formatDateTime, formatMonthDay } from "../../lib/format-datetime";
import { formatClock } from "../../lib/format-duration";
import type { PointerDragHandle } from "../../lib/pointer-drag";
import { isDisplacedCard, stageDropId, stageLabelKey, type BoardColumn } from "./creator-board";
import { channelNameById, useChannels } from "./use-channels";

export interface CreatorViewProps {
  columns: BoardColumn[];
  /** 显示的列（按用户的列序）——表格的阶段下拉只给这些。 */
  stages: readonly ProjectStageDto[];
  drag: PointerDragHandle<ProjectCardDto>;
  /** 拖拽经过的那一列 / 那一组。 */
  hoverStage: ProjectStageDto | null;
  /** 正在写盘的那一张（Rust 回答之前不让它再动）。 */
  moving: string | null;
  onCardClick: (card: ProjectCardDto) => void;
  onCardMenu: (event: ReactMouseEvent, card: ProjectCardDto) => void;
  onColumnMenu: (event: ReactMouseEvent, stage: ProjectStageDto) => void;
  onMove: (card: ProjectCardDto, stage: ProjectStageDto) => void;
}

/** 卡片 / 行上的「停在：<阶段>」（它的阶段那一列被收起了）。 */
function DisplacedStage({ card }: { card: ProjectCardDto }) {
  const { t } = useTranslation("workspace");
  return isDisplacedCard(card) ? (
    <span className="luna-creator__card-stage">
      {t("creator.atStage", { stage: t(stageLabelKey(card.stage)) })}
    </span>
  ) : null;
}

/** 渠道名（与发布页签的渠道胶囊同一组词）。 */
function channelNames(card: ProjectCardDto, catalog: readonly ChannelDto[], t: TFunction): string {
  return card.channels.map((id) => channelNameById(id, catalog, t)).join(" · ");
}

/**
 * 卡片上的信号（PRD-19-040）：计划发布日期、平台、口播时长、引文数、发布进度，下一行是下一步。
 * 每一项是一个独立的小块，没有的就不出现（没排期不写「未排期」，没有脚本不写「0:00」）。
 */
export function CardSignals({ card }: { card: ProjectCardDto }) {
  const { t, i18n } = useTranslation("workspace");
  const catalog = useChannels();
  const { signals } = card;
  const date = formatMonthDay(card.publish_date, i18n.language);
  return (
    <>
      <span className="luna-creator__card-meta">
        {date ? <span>{t("creator.card.date", { date })}</span> : null}
        <span>{channelNames(card, catalog, t)}</span>
        {signals.script_seconds !== null && signals.script_seconds > 0 ? (
          <span>{t("creator.card.script", { duration: formatClock(signals.script_seconds * 1000) })}</span>
        ) : null}
        {signals.quotes > 0 ? <span>{t("creator.card.quotes", { count: signals.quotes })}</span> : null}
        {signals.publish_done > 0 ? (
          <span>{t("creator.card.publish", { done: signals.publish_done, total: signals.publish_total })}</span>
        ) : null}
      </span>
      {signals.next_action ? (
        <span className="luna-creator__card-next">
          {t("creator.card.next", { action: t(`creator.nextAction.${signals.next_action}`) })}
        </span>
      ) : null}
    </>
  );
}

function beginDrag(drag: PointerDragHandle<ProjectCardDto>, card: ProjectCardDto) {
  return (event: ReactPointerEvent) => {
    if (event.button === 0) {
      drag.begin(event, card);
    }
  };
}

/** 看板：按阶段分列，拖动卡片到另一列即改阶段。 */
export function BoardColumnsView({
  columns,
  drag,
  hoverStage,
  moving,
  onCardClick,
  onCardMenu,
  onColumnMenu,
}: CreatorViewProps) {
  const { t } = useTranslation("workspace");
  return (
    <div className="luna-creator__board" role="list" aria-label={t("creator.boardLabel")}>
      {columns.map((column) => (
        <section
          key={column.stage}
          role="listitem"
          className="luna-creator__column"
          aria-labelledby={`luna-creator-col-${column.stage}`}
          data-drop-target={hoverStage === column.stage || undefined}
          data-drop-id={stageDropId(column.stage)}
        >
          <header
            className="luna-creator__column-header"
            onContextMenu={(event) => onColumnMenu(event, column.stage)}
          >
            <h2 id={`luna-creator-col-${column.stage}`}>{t(stageLabelKey(column.stage))}</h2>
            <span className="luna-count-badge">{column.cards.length}</span>
          </header>
          <ul className="luna-creator__cards">
            {column.cards.map((card) => (
              <li key={card.path} className="luna-creator__card-row">
                <button
                  type="button"
                  className="luna-creator__card"
                  title={card.path}
                  data-dragging={drag.payload?.path === card.path || undefined}
                  disabled={moving === card.path}
                  onPointerDown={beginDrag(drag, card)}
                  onClick={() => onCardClick(card)}
                  onContextMenu={(event) => onCardMenu(event, card)}
                >
                  <span className="luna-creator__card-title">{card.title}</span>
                  <DisplacedStage card={card} />
                  <CardSignals card={card} />
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

/**
 * 列表：按阶段分组的紧凑清单（Linear 的 List、Notion 的分组列表）。组可折叠，组头右键与看板
 * 列头同一份菜单；把一行拖到另一组的组头或组内即改阶段。折叠只是这一次查看的界面状态，不落盘。
 */
export function ListView({
  columns,
  drag,
  hoverStage,
  moving,
  onCardClick,
  onCardMenu,
  onColumnMenu,
}: CreatorViewProps) {
  const { t, i18n } = useTranslation("workspace");
  const [collapsed, setCollapsed] = useState<ReadonlySet<ProjectStageDto>>(new Set());
  const toggle = (stage: ProjectStageDto) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(stage)) {
        next.delete(stage);
      } else {
        next.add(stage);
      }
      return next;
    });
  return (
    <div className="luna-creator__list" aria-label={t("creator.boardLabel")}>
      {columns.map((column) => {
        const open = !collapsed.has(column.stage);
        const groupId = `luna-creator-group-${column.stage}`;
        return (
          <section
            key={column.stage}
            className="luna-creator__group"
            aria-labelledby={groupId}
            data-drop-target={hoverStage === column.stage || undefined}
            data-drop-id={stageDropId(column.stage)}
          >
            <button
              type="button"
              id={groupId}
              className="luna-creator__group-header"
              aria-expanded={open}
              onClick={() => toggle(column.stage)}
              onContextMenu={(event) => onColumnMenu(event, column.stage)}
            >
              {open ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
              <span className="luna-creator__group-title">{t(stageLabelKey(column.stage))}</span>
              <span className="luna-count-badge">{column.cards.length}</span>
            </button>
            {open && column.cards.length > 0 ? (
              <ul className="luna-creator__rows">
                {column.cards.map((card) => (
                  <li key={card.path} className="luna-creator__card-row">
                    <button
                      type="button"
                      className="luna-creator__row"
                      title={card.path}
                      data-dragging={drag.payload?.path === card.path || undefined}
                      disabled={moving === card.path}
                      onPointerDown={beginDrag(drag, card)}
                      onClick={() => onCardClick(card)}
                      onContextMenu={(event) => onCardMenu(event, card)}
                    >
                      <span className="luna-creator__card-title">{card.title}</span>
                      <DisplacedStage card={card} />
                      <span className="luna-creator__card-time">
                        {formatDateTime(card.modified_ms, i18n.language)}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            ) : null}
          </section>
        );
      })}
    </div>
  );
}

/**
 * 表格：一行一个项目（Notion 表格、GitHub Projects 的 Table）。阶段格是下拉，选了即改；
 * 停在收起列里的项目，下拉里额外列出它真实的阶段（标「已收起」），不让它被静默改成别的。
 * 行序与 Rust 给的一致（主页修改时间倒序），不在前端另排。
 */
export function TableView({ columns, stages, moving, onCardClick, onCardMenu, onMove }: CreatorViewProps) {
  const { t, i18n } = useTranslation("workspace");
  const rows = columns.flatMap((column) => column.cards);
  return (
    <div className="luna-creator__table-wrap">
      <table className="luna-creator__table" aria-label={t("creator.boardLabel")}>
        <thead>
          <tr>
            <th scope="col">{t("creator.table.title")}</th>
            <th scope="col">{t("creator.table.stage")}</th>
            <th scope="col">{t("creator.table.date")}</th>
            <th scope="col">{t("creator.table.script")}</th>
            <th scope="col">{t("creator.table.publish")}</th>
            <th scope="col">{t("creator.table.folder")}</th>
            <th scope="col">{t("creator.table.modified")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((card) => (
            <tr key={card.path} onContextMenu={(event) => onCardMenu(event, card)}>
              <td>
                <button type="button" className="luna-creator__table-title" title={card.path} onClick={() => onCardClick(card)}>
                  {card.title}
                </button>
              </td>
              <td>
                <SelectBox
                  size="sm"
                  ariaLabel={t("creator.table.stageOf", { title: card.title })}
                  value={card.stage}
                  disabled={moving === card.path}
                  onChange={(event) => onMove(card, event.target.value as ProjectStageDto)}
                >
                  {stages.includes(card.stage) ? null : (
                    <option value={card.stage}>
                      {t("creator.table.hiddenStage", { stage: t(stageLabelKey(card.stage)) })}
                    </option>
                  )}
                  {stages.map((stage) => (
                    <option key={stage} value={stage}>
                      {t(stageLabelKey(stage))}
                    </option>
                  ))}
                </SelectBox>
              </td>
              <td className="luna-creator__table-muted">{formatMonthDay(card.publish_date, i18n.language)}</td>
              <td className="luna-creator__table-muted">
                {card.signals.script_seconds ? formatClock(card.signals.script_seconds * 1000) : ""}
              </td>
              <td className="luna-creator__table-muted">
                {card.signals.publish_total > 0 ? `${card.signals.publish_done}/${card.signals.publish_total}` : ""}
              </td>
              <td className="luna-creator__table-muted">{card.folder}</td>
              <td className="luna-creator__table-muted">{formatDateTime(card.modified_ms, i18n.language)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
