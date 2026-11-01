/**
 * 归档清单（录音 / 转写）共用的「搜索 + 排序」一行（PRD-07-007 / TDD-07 §4.1a）。
 *
 * **它只是在告诉 Rust 怎么取这份列表**：筛选词与排序键归工作区壳持有，本组件不筛、不排、
 * 不知道条目——两张清单此前一张有这一行、一张没有，同基座的两个消费者能力不对等，而不对等
 * 的方向还错了（录音才是回访最频繁的一类）。现在两张清单各挂一份同一个组件。
 */

import { useTranslation } from "react-i18next";
import type { ListSortDto } from "../../ipc/bindings";
import { QueryBox } from "../../components/ui/QueryBox";
import { SelectBox } from "../../components/ui/SelectBox";
import { TRANSCRIBE_SORT_OPTIONS } from "./transcribe-list";

export interface ArchiveListFiltersProps {
  query: string;
  onQueryChange: (query: string) => void;
  sort: ListSortDto;
  onSortChange: (sort: ListSortDto) => void;
  /** 搜索框占位（同时是它的可访问名）：由调用方按对象类给（「搜索录音」/「搜索转写」）。 */
  searchLabel: string;
  /** 个别排序档的用词按对象类改（录音的 `created` 叫「录制时间」，转写叫「创建时间」）。 */
  sortLabels?: Partial<Record<ListSortDto, string>>;
}

export function ArchiveListFilters({
  query,
  onQueryChange,
  sort,
  onSortChange,
  searchLabel,
  sortLabels,
}: ArchiveListFiltersProps) {
  const { t } = useTranslation("transcribe");
  return (
    <div className="luna-transcribe-filters">
      {/* 查询框与排序下拉都走全应用的表单原语（`QueryBox` / `SelectBox`）：两者是同构的
          「外框 + 内部裸控件」，同一行里**等高由结构保证**——此前下拉是套着 `.luna-input`
          的原生 <select>，它忽略 `line-height`、自带最小高度，比查询框矮 7px，靠一句手调
          `padding-block` 去凑，而那句在舒适密度档下会再次错位（真机 2026-09-08）。 */}
      <QueryBox
        value={query}
        onChange={onQueryChange}
        placeholder={searchLabel}
        ariaLabel={searchLabel}
        className="luna-transcribe-filters__search"
      />
      <SelectBox
        className="luna-transcribe-filters__sort"
        value={sort}
        ariaLabel={t("sortLabel")}
        onChange={(event) => onSortChange(event.target.value as ListSortDto)}
      >
        {TRANSCRIBE_SORT_OPTIONS.map(([key, labelKey]) => (
          <option key={key} value={key}>
            {sortLabels?.[key] ?? t(labelKey)}
          </option>
        ))}
      </SelectBox>
    </div>
  );
}
