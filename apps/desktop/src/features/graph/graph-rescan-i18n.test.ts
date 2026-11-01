import { describe, expect, it } from "vitest";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import zhNotifications from "../../i18n/locales/zh-CN/notifications.json";
import enNotifications from "../../i18n/locales/en-US/notifications.json";
import {
  MATURITY_FILTER_GROUP,
  NOTE_TYPE_FILTERS,
  NOTE_TYPE_GROUPS,
} from "./graph-sidebar-config";
import { GRAPH_NOTE_TYPES } from "./graph-sim";
import { noteTypeLabelKey } from "../../lib/note-type";
import { comparableLocaleKeys, type LocaleDict } from "../../i18n/locale-keys";

/**
 * 全库重扫（PRD-06-035 全库重扫入口）的 i18n 守护。
 *
 * ① 通知 kind 是 **Rust 侧字符串**（`graph.rs` 的 `notify_rescan_outcome` 发
 * `graph_tags_rescanned` / `graph_tags_rescan_halted`），前端按
 * `notifications:<kind>.{title,body}` 动态查表——静态扫描 `t("…")` 的正则看不到，
 * 漏写文案时通知中心显示的是通用兜底而非这件事本身，且全部门禁皆绿。
 * ② `graph.*` 键集中英必须一致——重扫文案是本批次新增的一整片，漏一边就是裸 key。
 */

type Dict = Record<string, unknown>;

function lookup(bundle: Dict, path: string): string | undefined {
  const value = path.split(".").reduce<unknown>((node, key) => {
    return typeof node === "object" && node !== null ? (node as Dict)[key] : undefined;
  }, bundle);
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

describe("全库重扫的动态文案（PRD-06-035）", () => {
  it("Rust 发出的两个通知 kind 在中英通知文案里都在场", () => {
    // kind 是 Rust `notify_rescan_outcome` 的镜像；改动一侧另一侧必须跟上。
    for (const kind of ["graph_tags_rescanned", "graph_tags_rescan_halted"]) {
      for (const bundle of [zhNotifications, enNotifications] as unknown as Dict[]) {
        expect(lookup(bundle, `${kind}.title`), `缺 ${kind}.title`).toBeTruthy();
        expect(lookup(bundle, `${kind}.body`), `缺 ${kind}.body`).toBeTruthy();
      }
    }
  });

  /**
   * 批处理中止的文案必须**跟着真实落点走**，不得写死某个引擎（ADR-059 ③）。
   *
   * 真机坐实（2026-08-11）：连着组织服务端跑关系标签重扫，中止通知写着「请检查本机 AI
   * 引擎是否在运行」——而那台机器上标签根本不由本机生成。用户照着这句话去查一个与故障
   * 毫无关系的东西，而真正的原因（服务端不提供该能力）没有任何提示。
   *
   * Rust 侧 `NotificationEvent::executed_at` 把落点作为 i18next `context` 发过来，
   * 前端 `t(key, params)` 于是取到 `body_<落点>`。四种落点缺任一条都会静默退回通用
   * `body`——那正是「文案没写」最难被发现的形态，故这里逐条钉死。
   */
  it("批处理中止文案按真实落点分档，且不写死引擎名（ADR-059）", () => {
    const HALT_KINDS = [
      "graph_tags_rescan_halted",
      "knowledge_checkup_halted",
      "semantic_index_halted",
    ];
    // 与 Rust `ComputeLocation::name()` 逐字一致；那边加一种落点、这边即红。
    const LOCATIONS = ["strict_local", "on_device", "official_cloud", "organization"];
    // 本机两档文案指认本机引擎用的字眼；服务端两档与兜底出现它就是把用户支错了地方。
    const LOCAL_ENGINE = /AI 引擎|AI engine/i;
    for (const kind of HALT_KINDS) {
      for (const bundle of [zhNotifications, enNotifications] as unknown as Dict[]) {
        for (const location of LOCATIONS) {
          expect(
            lookup(bundle, `${kind}.body_${location}`),
            `缺 ${kind}.body_${location}——该落点下会退回通用文案，说不出为什么失败`,
          ).toBeTruthy();
        }
        // 服务端两档绝不能提本机引擎：那是把用户支向另一台机器。
        for (const location of ["official_cloud", "organization"]) {
          expect(
            lookup(bundle, `${kind}.body_${location}`),
            `${kind}.body_${location} 提到了本机引擎`,
          ).not.toMatch(LOCAL_ENGINE);
        }
        // 落点未知时的通用兜底同样不得指认任何一方。
        expect(
          lookup(bundle, `${kind}.body`),
          `${kind}.body 是落点未知时的兜底，不该指认某个引擎`,
        ).not.toMatch(LOCAL_ENGINE);
      }
    }
  });

  it("蒸馏提示通知的 kind 在中英文案里都在场（ADR-024）", () => {
    // kind 由 Rust `notify_processing_done` 在「完成后提示」档换出（`*_distill`），
    // 前端按 `notifications:<kind>.{title,body}` 动态查表——漏写文案时通知里
    // 显示的是兜底而非"可生成知识笔记"这件事，且全部门禁皆绿。
    // 注：曾有 `distill_batch_done` / `distill_batch_halted` 两个 kind，随批量自建 runner
    // 并入统一处理队列（PRD-06-051 ⑩）一起退场——守护一个不存在的发出方是死断言。
    for (const kind of [
      "recording_retranscribed_distill",
      "transcribe_fetched_distill",
      "chat_ended_distill",
      // 旧版「主题页已合成」：发出方已随入队合成一起退役（新建主题页改为先预览、确认后
      // 发布），但通知历史里的旧行仍按这个 kind 查文案。
      "topic_composed",
      // Auto 档归入完成（PRD-16-004/024）。发出方是 `ingest.rs::run_ingest` **自己**，
      // 不在 `notify_processing_done` 的 tag 表里——只有它知道「这一轮到底写没写进去」，
      // 而「算完发现无页可归」绝不该弹一条「知识库已更新」。
      "knowledge_ingested",
      // 语义索引全量建库的终局（PRD-16-028，`embed.rs::notify_outcome` 的两个分支）。
      // 建库是十分钟级的后台任务（S16：万片约 40 分钟），终局必须能把用户带回来。
      "semantic_index_built",
      "semantic_index_halted",
      // 知识体检（PRD-16-031/032）：深度体检的两个终局分支（`checkup.rs::notify_outcome`）
      // 与周期提醒（`maybe_remind`）。提醒那条尤其容易漏——它默认关，开发期几乎不会自然
      // 触发一次，于是"文案没写"要等到某个真实用户开了开关两周后才暴露。
      "knowledge_checkup_done",
      "knowledge_checkup_halted",
      "knowledge_checkup_due",
    ]) {
      for (const bundle of [zhNotifications, enNotifications] as unknown as Dict[]) {
        expect(lookup(bundle, `${kind}.title`), `缺 ${kind}.title`).toBeTruthy();
        expect(lookup(bundle, `${kind}.body`), `缺 ${kind}.body`).toBeTruthy();
      }
    }
  });

  it("workspace 全文件键集中英一致（递归）", () => {
    // 原本只锁 graph 区段；多库切换（vault.*）与分层可感知化（tree.* / distill.*）
    // 又各加了一整片动态查表的键——逐区段登记必然漏掉下一片，改为递归锁整个文件。
    // 取键集与去复数后缀的判据归 `i18n/locale-keys`——全 namespace 那份守护用的是同一
    // 份，两边各写一份 `flatten` 迟早只有一份跟上变化，而它们守的恰恰是"两边必须一样"。
    expect(comparableLocaleKeys(zhWorkspace as unknown as LocaleDict)).toEqual(
      comparableLocaleKeys(enWorkspace as unknown as LocaleDict),
    );
  });

  it("笔记类型过滤器覆盖全部类型且文案在场（ADR-024）", () => {
    // 勾选组必须逐一覆盖 `GRAPH_NOTE_TYPES`——漏一类即那类笔记永远无法在图谱上显示，
    // 而画面上只是"少了点东西"，没有任何报错。
    expect(NOTE_TYPE_FILTERS.map((entry) => entry.type)).toEqual([...GRAPH_NOTE_TYPES]);
    // 每一类都要归进某一组，否则那一类的开关根本不会被渲染（PRD-06-048③）。
    const groupIds = new Set(NOTE_TYPE_GROUPS.map((g) => g.id));
    for (const entry of NOTE_TYPE_FILTERS) {
      expect(groupIds.has(entry.group), `类型 ${entry.type} 未归组`).toBe(true);
    }
    for (const group of NOTE_TYPE_GROUPS) {
      for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
        expect(lookup(bundle, `graph.${group.label}`), `缺 graph.${group.label}`).toBeTruthy();
      }
    }
    // 类型译名落在中性的 `noteType.*`（不带 `graph.` 前缀）：右侧上下文面板的
    // 「笔记信息」说的是同一句话，而在一个与图谱无关的面板里写 `t("graph.typeQuickNote")`
    // 只会诱导下一个人另抄一份译名。故这里直接按**完整 key** 判，不再假设前缀。
    for (const entry of NOTE_TYPE_FILTERS) {
      // 判据落在有依赖的这一侧：`graph` 本就依赖 `lib`，而反过来让 `workspace` 引
      // `graph` 的注册表会构成功能模块循环依赖。
      expect(entry.label, `${entry.type} 的译名没走中性 helper`).toBe(
        noteTypeLabelKey(entry.type),
      );
      for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
        expect(lookup(bundle, entry.label), `缺 ${entry.label}`).toBeTruthy();
      }
    }
  });

  it("记录开关**不在**类型清单里，而挂在某个真实小组下（ADR-086 成熟度轴）", () => {
    // 成熟度与来源正交：混进 `NOTE_TYPE_FILTERS` 会当场教会用户一句错话
    // 「记录是第六种笔记类型」，而事实是一篇记录既可以是 note 也可以是速记。
    const typeLabels: readonly string[] = NOTE_TYPE_FILTERS.map((entry) => entry.label);
    expect(typeLabels).not.toContain("showDrafts");
    expect(NOTE_TYPE_GROUPS.some((group) => group.id === MATURITY_FILTER_GROUP)).toBe(true);
    for (const key of ["maturity", "showDrafts", "showDraftsHint"]) {
      for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
        expect(lookup(bundle, `graph.${key}`), `缺 graph.${key}`).toBeTruthy();
      }
    }
  });
});
