// @vitest-environment happy-dom
/**
 * 持续整理面板的**渲染行为**（ADR-134 / TDD-07 §4.1e）。
 *
 * 判据是渲染与回传的参数，不是源码字符串：这一格的风险全在「给了哪个入口」「确认时
 * 带回的是不是用户看到的那一份」「迟到响应会不会盖掉新选择」——三者在源码扫描下都不可见。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TopicEvidencePanel } from "./TopicEvidencePanel";
import { forgetApplyUndo } from "./apply-undo-memory";

const TOPIC = "Knowledge/项目.md";

afterEach(cleanup);
beforeEach(resetBindingsDouble);
// 撤销凭据是会话级内存：用例之间清掉，免得上一个用例的写入冒到下一个里。
beforeEach(() => forgetApplyUndo(TOPIC));

function detail(
  maintenance: string,
  extra: {
    pending?: unknown;
    pending_operation_id?: string | null;
    apply_phase?: "idle" | "queued" | "failed";
    transcript_sources?: { authorized: boolean; available: number };
    background_preparing?: boolean;
  } = {},
) {
  return {
    status: "ok",
    data: {
      maintenance,
      pending: null,
      pending_operation_id: null,
      apply_phase: "idle",
      transcript_sources: { authorized: false, available: 0 },
      background_preparing: false,
      ...extra,
    },
  };
}

function mount(path = TOPIC, onOpenPath = vi.fn(), onOpenHistory = vi.fn()) {
  return render(withI18n(createElement(TopicEvidencePanel, { path, onOpenPath, onOpenHistory })));
}

const PLANNED = {
  status: "ok",
  data: {
    status: "planned",
    duplicates: 1,
    input_complete: false,
    omitted_sources: 2,
    preview: {
      status: "ready",
      operation_id: "op-1",
      target_path: TOPIC,
      before_hash: "hash-0",
      result_hash: "hash-1",
      markdown: "# 项目\n",
      items: [
        {
          action: "append",
          text: "原型已进入内测",
          sources: [{ path: "Notes/周会.md", title: "周会", evidence_index: 0 }],
          transcript_record: false,
          quick_note_record: false,
          related_text: null,
        },
        {
          action: "conflict",
          text: "发布推迟到下月",
          sources: [{ path: "Notes/周会.md", title: "周会", evidence_index: 0 }],
          transcript_record: false,
          quick_note_record: false,
          related_text: "项目本周发布",
        },
      ],
      ready_item_count: { kind: "exact", count: 2 },
      evidence_count: 1,
    },
  },
};

describe("TopicEvidencePanel", () => {
  it("只在维护就绪或旧格式时给入口，其余状态不渲染", async () => {
    setCommandReply("topicCurationStatus", detail("body_changed"));
    const { container } = mount();
    await vi.waitFor(() =>
      expect(commandMock("topicCurationStatus")).toHaveBeenCalledWith(TOPIC),
    );
    expect(container.querySelector(".luna-topic-evidence")).toBeNull();
    cleanup();

    setCommandReply("topicCurationStatus", detail("needs_upgrade"));
    mount();
    expect(await screen.findByText(t("workspace:topicEvidence.upgrade"))).toBeTruthy();
  });

  it("自动补充停止时如实说明原因，不给检查或解除保护的入口", async () => {
    for (const state of ["metadata_changed", "missing_baseline", "unknown_schema", "invalid_evidence"]) {
      setCommandReply("topicCurationStatus", detail(state));
      mount();
      expect(await screen.findByText(t(`workspace:topicEvidence.paused.${state}`))).toBeTruthy();
      expect(screen.queryByText(t("workspace:topicEvidence.check"))).toBeNull();
      cleanup();
    }
  });

  it("暂停时直接给版本历史这个修复出口；未知新版格式只让用户更新应用", async () => {
    setCommandReply("topicCurationStatus", detail("invalid_evidence"));
    const onOpenHistory = vi.fn();
    mount(TOPIC, vi.fn(), onOpenHistory);
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.openHistory")));
    expect(onOpenHistory).toHaveBeenCalled();
    cleanup();
    setCommandReply("topicCurationStatus", detail("unknown_schema"));
    mount();
    expect(await screen.findByText(t("workspace:topicEvidence.paused.unknown_schema"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicEvidence.openHistory"))).toBeNull();
  });

  it("回到主题时接回盘上已封存的计划，不重跑模型", async () => {
    setCommandReply(
      "topicCurationStatus",
      detail("ready", { pending: PLANNED.data, pending_operation_id: "op-1" }),
    );
    mount();
    expect(await screen.findByText("原型已进入内测")).toBeTruthy();
    expect(commandMock("prepareTopicIngestPlan")).not.toHaveBeenCalled();
  });

  it("接回的计划已过期时说明原因并可丢弃；丢弃不承诺「不再推荐」", async () => {
    const stale = {
      ...PLANNED.data,
      preview: {
        status: "rejected",
        rejection: { code: "source_changed", source_path: null, target_state: null, can_replan: true },
      },
    };
    setCommandReply(
      "topicCurationStatus",
      detail("ready", { pending: stale, pending_operation_id: "op-old" }),
    );
    setCommandReply("decideSealedIngestPlan", {
      status: "ok",
      data: {
        status: "discarded",
        rejection: { code: "source_changed", source_path: null, target_state: null, can_replan: true },
      },
    });
    const directoryStale = vi.fn();
    window.addEventListener("luna:topic-directory-stale", directoryStale);
    mount();
    expect(
      await screen.findByText(t("workspace:topicEvidence.rejection.source_changed")),
    ).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.discardStale")));
    expect(commandMock("decideSealedIngestPlan")).toHaveBeenCalledWith("op-old", "ignored");
    expect(await screen.findByText(t("workspace:topicEvidence.discarded"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicEvidence.ignored"))).toBeNull();
    // 过期计划没有记抑制，候选数不变，不必让目录重取。
    expect(directoryStale).not.toHaveBeenCalled();
    window.removeEventListener("luna:topic-directory-stale", directoryStale);
  });

  it("来源或本页变了导致计划失效时一键重新检查；重跑无意义的原因不给", async () => {
    const stale = {
      ...PLANNED.data,
      preview: {
        status: "rejected",
        rejection: { code: "target_changed", source_path: null, target_state: null, can_replan: true },
      },
    };
    setCommandReply(
      "topicCurationStatus",
      detail("ready", { pending: stale, pending_operation_id: "op-old" }),
    );
    setCommandReply("prepareTopicIngestPlan", PLANNED);
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.replan")));
    expect(commandMock("prepareTopicIngestPlan")).toHaveBeenCalledWith(TOPIC);
    expect(await screen.findByText("原型已进入内测")).toBeTruthy();
    cleanup();

    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", {
      status: "ok",
      data: {
        status: "rejected",
        rejection: { code: "user_suppressed", source_path: null, target_state: null, can_replan: false },
      },
    });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(
      await screen.findByText(t("workspace:topicEvidence.rejection.user_suppressed")),
    ).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicEvidence.replan"))).toBeNull();
  });

  it("检查失败时说出共用的成因句（如登录失效），认不出的错误仍用本面板的中性句", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", { status: "error", error: "SIGN_IN_REQUIRED" });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(await screen.findByText(t("common:failure.SIGN_IN_REQUIRED"))).toBeTruthy();
    cleanup();

    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", { status: "error", error: "INGEST_NO_RESULT: 细节" });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(await screen.findByText(t("workspace:topicEvidence.failed"))).toBeTruthy();
    expect(screen.queryByText(/INGEST_NO_RESULT/)).toBeNull();
  });

  it("有可用的权威转写且未授权时，先说明影响再请求一次确认", async () => {
    setCommandReply(
      "topicCurationStatus",
      detail("ready", { transcript_sources: { authorized: false, available: 2 } }),
    );
    setCommandReply("setTranscriptSources", { status: "ok", data: null });
    mount();
    expect(
      await screen.findByText(t("workspace:topicEvidence.transcripts.available", { count: 2 })),
    ).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.transcripts.ask")));
    expect(screen.getByText(t("workspace:topicEvidence.transcripts.impact"))).toBeTruthy();
    expect(screen.getByText(t("workspace:topicEvidence.transcripts.scope"))).toBeTruthy();
    // 「暂不」不写任何授权。
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.transcripts.notNow")));
    expect(commandMock("setTranscriptSources")).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.transcripts.ask")));
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.transcripts.allow")));
    expect(commandMock("setTranscriptSources")).toHaveBeenCalledWith("allow");
  });

  it("已授权时可撤回；没有可用转写时不打扰", async () => {
    setCommandReply(
      "topicCurationStatus",
      detail("ready", { transcript_sources: { authorized: true, available: 0 } }),
    );
    setCommandReply("setTranscriptSources", { status: "ok", data: null });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.transcripts.revoke")));
    expect(commandMock("setTranscriptSources")).toHaveBeenCalledWith("revoke");
    cleanup();
    setCommandReply("topicCurationStatus", detail("ready"));
    mount();
    await screen.findByText(t("workspace:topicEvidence.check"));
    expect(screen.queryByText(t("workspace:topicEvidence.transcripts.ask"))).toBeNull();
  });

  it("转写记载条目标出它的身份，而不是只说「新增」", async () => {
    const plan = structuredClone(PLANNED);
    plan.data.preview.items = [
      {
        action: "append",
        text: "灰度十月二十日结束",
        sources: [{ path: "Transcripts/灰度会议.md", title: "灰度会议", evidence_index: 0 }],
        transcript_record: true,
        quick_note_record: false,
        related_text: null,
      },
    ];
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", plan);
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(await screen.findByText("灰度十月二十日结束")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicEvidence.action.transcript"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicEvidence.action.append"))).toBeNull();
  });

  it("只由速记支撑的条目标「速记记载」", async () => {
    const plan = structuredClone(PLANNED);
    plan.data.preview.items = [
      {
        action: "append",
        text: "客户希望支持批量导入",
        sources: [{ path: "QuickNotes/客户访谈.md", title: "客户访谈", evidence_index: 0 }],
        transcript_record: false,
        quick_note_record: true,
        related_text: null,
      },
    ];
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", plan);
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(await screen.findByText("客户希望支持批量导入")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicEvidence.action.quickNote"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicEvidence.action.append"))).toBeNull();
  });

  it("后台正在准备时不给会重复调模型的检查按钮；准备完成后接回计划", async () => {
    setCommandReply("topicCurationStatus", detail("ready", { background_preparing: true }));
    mount();
    expect(await screen.findByText(t("workspace:topicEvidence.backgroundPreparing"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicEvidence.check"))).toBeNull();
    // 本页那一件仍在跑：不重读现场。
    const calls = commandMock("topicCurationStatus").mock.calls.length;
    emitEvent("processingQueue", {
      items: [
        {
          id: "1",
          kind: "topic_prepare",
          label: "项目",
          note_rel: TOPIC,
          status: { state: "running", data: { step: "filing", done: 0, total: 1 } },
          executed_by: null,
          queue: null,
          restored: false,
        },
      ],
    });
    expect(commandMock("topicCurationStatus").mock.calls.length).toBe(calls);
    // 跑完：重读现场，准备好的计划直接出现。
    setCommandReply(
      "topicCurationStatus",
      detail("ready", { pending: PLANNED.data, pending_operation_id: "op-1" }),
    );
    emitEvent("processingQueue", { items: [] });
    expect(await screen.findByText("原型已进入内测")).toBeTruthy();
    expect(commandMock("prepareTopicIngestPlan")).not.toHaveBeenCalled();
  });

  it("有应用在排队时直接显示已加入队列", async () => {
    setCommandReply("topicCurationStatus", detail("ready", { apply_phase: "queued" }));
    mount();
    expect(await screen.findByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
  });

  it("已加入队列后跟着队列广播落定：成功说已写入，失败带说明接回同一份计划", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", PLANNED);
    setCommandReply("confirmSealedIngestPlan", {
      status: "ok",
      data: { status: "queued", operation_id: "op-1", target_path: TOPIC },
    });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.apply")));
    expect(await screen.findByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
    // 还在排队：广播不改变界面。
    setCommandReply("topicCurationStatus", detail("ready", { apply_phase: "queued" }));
    emitEvent("processingQueue", { items: [] });
    await vi.waitFor(() => expect(commandMock("topicCurationStatus")).toHaveBeenCalledTimes(2));
    expect(screen.getByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
    // 失败：同一份计划回到待确认，并说明上次没有写入。
    setCommandReply(
      "topicCurationStatus",
      detail("ready", { pending: PLANNED.data, pending_operation_id: "op-1", apply_phase: "failed" }),
    );
    emitEvent("processingQueue", { items: [] });
    expect(await screen.findByText(t("workspace:topicEvidence.applyFailed"))).toBeTruthy();
    expect(screen.getByText("原型已进入内测")).toBeTruthy();
    // 再次应用后成功：计划已清理，转成已写入。
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.apply")));
    expect(await screen.findByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
    setCommandReply("topicCurationStatus", detail("ready"));
    emitEvent("processingQueue", { items: [] });
    expect(await screen.findByText(t("workspace:topicEvidence.applied"))).toBeTruthy();
  });

  it("写入后可撤销：凭确认时的两个指纹恢复；之后又改过则不动并指向版本历史", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", PLANNED);
    setCommandReply("confirmSealedIngestPlan", {
      status: "ok",
      data: { status: "queued", operation_id: "op-1", target_path: TOPIC },
    });
    const onOpenHistory = vi.fn();
    mount(TOPIC, vi.fn(), onOpenHistory);
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.apply")));
    expect(await screen.findByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
    emitEvent("processingQueue", { items: [] });
    expect(await screen.findByText(t("workspace:topicEvidence.applied"))).toBeTruthy();

    setCommandReply("undoTopicApply", { status: "ok", data: { status: "changed" } });
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.undoApply")));
    expect(await screen.findByText(t("workspace:topicEvidence.undoChanged"))).toBeTruthy();
    expect(commandMock("undoTopicApply")).toHaveBeenCalledWith(TOPIC, "hash-0", "hash-1");
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.openHistory")));
    expect(onOpenHistory).toHaveBeenCalled();
  });

  it("离开主题再回来仍能撤销这次写入；撤销或关掉之后回来就不再给", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", PLANNED);
    setCommandReply("confirmSealedIngestPlan", {
      status: "ok",
      data: { status: "queued", operation_id: "op-1", target_path: TOPIC },
    });
    const first = mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.apply")));
    expect(await screen.findByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
    emitEvent("processingQueue", { items: [] });
    expect(await screen.findByText(t("workspace:topicEvidence.applied"))).toBeTruthy();
    first.unmount();

    // 回来：接着给撤销，凭的仍是确认时那两个指纹。
    const back = mount();
    setCommandReply("undoTopicApply", { status: "ok", data: { status: "restored" } });
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.undoApply")));
    expect(await screen.findByText(t("workspace:topicEvidence.undone"))).toBeTruthy();
    expect(commandMock("undoTopicApply")).toHaveBeenCalledWith(TOPIC, "hash-0", "hash-1");
    back.unmount();

    // 撤销过了：再回来只剩检查入口。
    mount();
    expect(await screen.findByText(t("workspace:topicEvidence.check"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicEvidence.undoApply"))).toBeNull();
  });

  it("替换与分歧在确认前摆出原来那一条", async () => {
    const plan = structuredClone(PLANNED);
    plan.data.preview.items = [
      {
        action: "replace",
        text: "项目改为下周发布",
        sources: [{ path: "Notes/周会.md", title: "周会", evidence_index: 0 }],
        transcript_record: false,
        quick_note_record: false,
        related_text: "项目本周发布",
      },
      ...PLANNED.data.preview.items,
    ];
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", plan);
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(await screen.findByText("项目改为下周发布")).toBeTruthy();
    expect(
      screen.getByText(t("workspace:topicEvidence.replacedText", { text: "项目本周发布" })),
    ).toBeTruthy();
    expect(
      screen.getByText(t("workspace:topicEvidence.relatedText", { text: "项目本周发布" })),
    ).toBeTruthy();
    // 新增那条没有原文，不摆空行。
    expect(screen.getAllByText(/项目本周发布/)).toHaveLength(2);
  });

  it("撤销成功后说明已恢复", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", PLANNED);
    setCommandReply("confirmSealedIngestPlan", {
      status: "ok",
      data: { status: "queued", operation_id: "op-1", target_path: TOPIC },
    });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.apply")));
    expect(await screen.findByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
    emitEvent("processingQueue", { items: [] });
    expect(await screen.findByText(t("workspace:topicEvidence.applied"))).toBeTruthy();
    setCommandReply("undoTopicApply", { status: "ok", data: { status: "restored" } });
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.undoApply")));
    expect(await screen.findByText(t("workspace:topicEvidence.undone"))).toBeTruthy();
  });

  it("从盘上接回的排队不知道是哪一份，写入后不给撤销", async () => {
    setCommandReply("topicCurationStatus", detail("ready", { apply_phase: "queued" }));
    mount();
    expect(await screen.findByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
    // 恢复态不跟队列落定（没有本次会话的操作 ID），也就不会冒出撤销。
    expect(screen.queryByText(t("workspace:topicEvidence.undoApply"))).toBeNull();
  });

  it("落定时盘上别的旧计划不冒充这次的结果", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", PLANNED);
    setCommandReply("confirmSealedIngestPlan", {
      status: "ok",
      data: { status: "queued", operation_id: "op-1", target_path: TOPIC },
    });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.apply")));
    await screen.findByText(t("workspace:topicEvidence.queued"));
    setCommandReply(
      "topicCurationStatus",
      detail("ready", { pending: PLANNED.data, pending_operation_id: "op-old" }),
    );
    emitEvent("processingQueue", { items: [] });
    expect(await screen.findByText(t("workspace:topicEvidence.applied"))).toBeTruthy();
  });

  it("计划逐条展示并以原 ID 与指纹确认；来源可打开", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", PLANNED);
    setCommandReply("confirmSealedIngestPlan", {
      status: "ok",
      data: { status: "queued", operation_id: "op-1", target_path: TOPIC },
    });
    const onOpenPath = vi.fn();
    mount(TOPIC, onOpenPath);
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(commandMock("prepareTopicIngestPlan")).toHaveBeenCalledWith(TOPIC);
    expect(await screen.findByText("原型已进入内测")).toBeTruthy();
    expect(screen.getByText("发布推迟到下月")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicEvidence.action.conflict"))).toBeTruthy();
    expect(screen.getByText(t("workspace:topicEvidence.partial", { count: 2 }))).toBeTruthy();
    expect(
      screen.getByText(t("workspace:topicEvidence.duplicatesSkipped", { count: 1 })),
    ).toBeTruthy();
    // 出处按封存定位锚交回 Rust 打开；计划失效（命令报错）时才退回按路径打开。
    setCommandReply("openSealedPlanSource", { status: "ok", data: "exact" });
    fireEvent.click(screen.getAllByText("周会")[0]);
    await vi.waitFor(() =>
      expect(commandMock("openSealedPlanSource")).toHaveBeenCalledWith("op-1", 0),
    );
    expect(onOpenPath).not.toHaveBeenCalled();
    setCommandReply("openSealedPlanSource", { status: "error", error: "PLAN_SOURCE_UNAVAILABLE" });
    fireEvent.click(screen.getAllByText("周会")[0]);
    await vi.waitFor(() => expect(onOpenPath).toHaveBeenCalledWith("Notes/周会.md"));

    fireEvent.click(screen.getByText(t("workspace:topicEvidence.apply")));
    expect(commandMock("confirmSealedIngestPlan")).toHaveBeenCalledWith("op-1", "hash-1");
    expect(await screen.findByText(t("workspace:topicEvidence.queued"))).toBeTruthy();
  });

  it("否决走随库决定命令；过期拒绝显示 Rust 原因码对应的文案", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", PLANNED);
    setCommandReply("decideSealedIngestPlan", {
      status: "ok",
      data: {
        status: "rejected",
        rejection: { code: "apply_pending", source_path: null, target_state: null, can_replan: false },
      },
    });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.reject")));
    expect(commandMock("decideSealedIngestPlan")).toHaveBeenCalledWith("op-1", "rejected");
    expect(
      await screen.findByText(t("workspace:topicEvidence.rejection.apply_pending")),
    ).toBeTruthy();
  });

  it("无新增与重复是结果，不是失败", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", {
      status: "ok",
      data: {
        status: "no_op",
        duplicates: 3,
        input_complete: true,
        omitted_sources: 0,
        more_to_read: false,
      },
    });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    const notice = await screen.findByText(
      t("workspace:topicEvidence.noOpDuplicates", { count: 3 }),
    );
    expect(notice.closest("[role='status']")).toBeTruthy();
  });

  it("只查了一批时不说「没有」：没轮到的来源与只读了一部分分开说", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    setCommandReply("prepareTopicIngestPlan", {
      status: "ok",
      data: {
        status: "no_op",
        duplicates: 0,
        input_complete: false,
        omitted_sources: 4,
        more_to_read: true,
      },
    });
    const view = mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(await screen.findByText(t("workspace:topicEvidence.noOpPartial"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicEvidence.noOp"))).toBeNull();
    view.unmount();

    setCommandReply("prepareTopicIngestPlan", {
      status: "ok",
      data: {
        status: "no_op",
        duplicates: 0,
        input_complete: false,
        omitted_sources: 0,
        more_to_read: true,
      },
    });
    const partlyRead = mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    // 没有遗漏来源，但超长来源只读了前半：Rust 说还有要读的，照样不能说「没有」。
    expect(await screen.findByText(t("workspace:topicEvidence.noOpPartial"))).toBeTruthy();
    partlyRead.unmount();

    setCommandReply("prepareTopicIngestPlan", {
      status: "ok",
      data: {
        status: "no_op",
        duplicates: 0,
        input_complete: false,
        omitted_sources: 0,
        more_to_read: false,
      },
    });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(await screen.findByText(t("workspace:topicEvidence.noOpClipped"))).toBeTruthy();
  });

  it("旧页升级：预览影响 → 带回前后指纹确认 → 可撤销", async () => {
    setCommandReply("topicCurationStatus", detail("needs_upgrade"));
    setCommandReply("previewTopicUpgrade", {
      status: "ok",
      data: [
        {
          status: "ready",
          path: TOPIC,
          before_hash: "before",
          after_hash: "after",
          scope_title: "项目 · A",
          scope_includes: ["项目/A"],
          user_edited: true,
          unverified_lines: 4,
        },
      ],
    });
    setCommandReply("applyTopicUpgrade", {
      status: "ok",
      data: [
        {
          status: "upgraded",
          path: TOPIC,
          backup_id: "backup-1",
          index_updated: true,
          baseline_recorded: false,
          maintenance: "body_changed",
        },
      ],
    });
    setCommandReply("restoreTopicUpgrade", {
      status: "ok",
      data: { status: "restored", path: TOPIC },
    });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.upgrade")));
    expect(
      await screen.findByText(
        t("workspace:topicEvidence.upgradeImpact", { scope: "项目 · A", count: 4 }),
      ),
    ).toBeTruthy();
    expect(screen.getByText(t("workspace:topicEvidence.upgradeUserEdited"))).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.upgradeConfirm")));
    expect(commandMock("applyTopicUpgrade")).toHaveBeenCalledWith([
      { path: TOPIC, before_hash: "before", after_hash: "after" },
    ]);
    expect(
      await screen.findByText(t("workspace:topicEvidence.upgradedReview")),
    ).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.upgradeUndo")));
    expect(commandMock("restoreTopicUpgrade")).toHaveBeenCalledWith("backup-1");
    expect(await screen.findByText(t("workspace:topicEvidence.upgradeRestored"))).toBeTruthy();
    // 关闭回执即按当前文件重取维护状态——入口不能停在升级前的旧判断上（真机 2026-09-24）。
    const loadsBeforeClose = commandMock("topicCurationStatus").mock.calls.length;
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.close")));
    await vi.waitFor(() =>
      expect(commandMock("topicCurationStatus").mock.calls.length).toBe(loadsBeforeClose + 1),
    );
  });

  it("切换到另一页后，上一页迟到的计划不会显示", async () => {
    setCommandReply("topicCurationStatus", detail("ready"));
    let release: (value: unknown) => void = () => {};
    setCommandReply(
      "prepareTopicIngestPlan",
      () => new Promise((resolve) => (release = resolve)),
    );
    const view = mount();
    fireEvent.click(await screen.findByText(t("workspace:topicEvidence.check")));
    expect(await screen.findByText(t("workspace:topicEvidence.preparing"))).toBeTruthy();
    view.rerender(
      withI18n(
        createElement(TopicEvidencePanel, {
          path: "Knowledge/另一页.md",
          onOpenPath: vi.fn(),
          onOpenHistory: vi.fn(),
        }),
      ),
    );
    release(PLANNED);
    await screen.findByText(t("workspace:topicEvidence.check"));
    expect(screen.queryByText("原型已进入内测")).toBeNull();
  });
});
