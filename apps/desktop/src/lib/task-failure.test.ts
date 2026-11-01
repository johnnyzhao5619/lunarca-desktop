import { describe, expect, it } from "vitest";
import zh from "../i18n/locales/zh-CN/transcribe.json";
import en from "../i18n/locales/en-US/transcribe.json";
import { fetchErrorKey, taskFailureText } from "./task-failure";
import zhCommon from "../i18n/locales/zh-CN/common.json";
import enCommon from "../i18n/locales/en-US/common.json";

/** 后端 `luna_fetch::FetchError::code` 的全集（新增变体必须同步——测试会红）。 */
const ALL_CODES = [
  "FETCH_TOOL_MISSING",
  "FETCH_TOOL_UNUSABLE",
  "FETCH_TOOL_OUTDATED",
  "FETCH_HOST_REJECTED",
  "FETCH_JS_RUNTIME_MISSING",
  "FETCH_UNSUPPORTED_URL",
  "FETCH_UNSUPPORTED_SITE",
  "FETCH_LOGIN_REQUIRED",
  "FETCH_GEO_BLOCKED",
  "FETCH_UNAVAILABLE",
  "FETCH_LIVE_STREAM",
  "FETCH_NETWORK",
  "FETCH_NO_COMPATIBLE_FORMAT",
  "FETCH_INSUFFICIENT_DISK",
  "FETCH_CANCELLED",
  "FETCH_TOOL_FAILED",
  "FETCH_NO_OUTPUT",
  "FETCH_BAD_PROBE_OUTPUT",
  "FETCH_IO",
  "STRICT_LOCAL_BLOCKED",
  // 受管安装（ADR-023 修订）
  "FETCH_TOOL_INSTALL_FAILED",
  "FETCH_TOOL_CHECKSUM_MISMATCH",
  "FETCH_TOOL_NO_CHECKSUM",
  "FETCH_TOOL_NETWORK",
  "FETCH_TOOL_UNINSTALL_FAILED",
];

describe("抓取错误码本地化（PRD-05-006）", () => {
  it("每个后端错误码在中英两份文案里都有对应句子", () => {
    for (const code of ALL_CODES) {
      const key = fetchErrorKey(code);
      // null = 未登记为抓取错误码 → 界面走中性通用失败文案，绝不回显原码。
      expect(key, `${code} 未登记映射`).not.toBeNull();
      const sub = (key as string).slice("fetchError.".length);
      // 文案缺失会让界面显示原始 key —— 那和显示错误码一样无用。
      expect((zh.fetchError as Record<string, string>)[sub], `zh 缺 ${sub}`).toBeTruthy();
      expect((en.fetchError as Record<string, string>)[sub], `en 缺 ${sub}`).toBeTruthy();
    }
  });

  /**
   * 队列面板里的失败**不只来自抓取**：远程转写的两个终态码借同一条本地化通道，
   * 它们同样必须有中英两句人话。
   *
   * 没有人话的代价不是「少一句提示」，而是这两个码**唯一能给出的下一步动作被抹掉**：
   * `QUEUE_EXPIRED` 的下一步是「换个时段再排一次，这次没计费」，`RESULT_UNAVAILABLE`
   * 的下一步是「重转一次，且那是一次新的计费」——回落到中性的 `failure.generic`
   * （「任务未能完成」）之后，两者与任何一种故障都长得一模一样。
   *
   * `RESULT_UNAVAILABLE` 这条尤其要紧：现网它的表现是**根本没有失败**——结果缺席被
   * 当成「转写出来是空的」写进了笔记，用户只看到一个「暂无转写内容」的空块。
   */
  it("远程转写的终态码同样有人话，而不是回落到通用失败", () => {
    const t = (key: string) => `T:${key}`;
    for (const [code, sub] of [
      ["QUEUE_EXPIRED", "queueExpired"],
      ["RESULT_UNAVAILABLE", "resultUnavailable"],
    ] as const) {
      expect(fetchErrorKey(code), `${code} 未登记映射`).toBe(`fetchError.${sub}`);
      expect((zh.fetchError as Record<string, string>)[sub], `zh 缺 ${sub}`).toBeTruthy();
      expect((en.fetchError as Record<string, string>)[sub], `en 缺 ${sub}`).toBeTruthy();
      expect(taskFailureText(code, t)).toBe(`T:fetchError.${sub}`);
    }
  });

  it("带尾注的错误串同样能认出（后端会附退出码 / IO 细节）", () => {
    expect(fetchErrorKey("FETCH_TOOL_FAILED (exit 1)")).toBe("fetchError.toolFailed");
    expect(fetchErrorKey("FETCH_IO: permission denied")).toBe("fetchError.io");
    expect(fetchErrorKey("  FETCH_NETWORK  ")).toBe("fetchError.network");
  });

  it("非抓取错误码返回 null——不把 DECODE_FAILED 谎报成「下载失败」", () => {
    // 队列面板里的失败不都来自抓取；一律说成"下载失败"是谎报，故这里必须是 null，
    // 由调用方决定：抓取语境兜到 unknown，通用语境退回显示原码。
    expect(fetchErrorKey("DECODE_FAILED")).toBeNull();
    expect(fetchErrorKey("TRANSLATE_NO_TRANSCRIPT")).toBeNull();
    expect(fetchErrorKey("")).toBeNull();
    expect((zh.fetchError as Record<string, string>).unknown).toBeTruthy();
  });

  it("展示文本：抓取码出人话，其余码走中性本地化文案", () => {
    const t = (key: string) => `T:${key}`;
    expect(taskFailureText("FETCH_LOGIN_REQUIRED", t)).toBe("T:fetchError.loginRequired");
    expect(taskFailureText("FETCH_TOOL_FAILED (exit 1)", t)).toBe("T:fetchError.toolFailed");
    expect(taskFailureText("DECODE_FAILED: broken container", t)).toBe("T:common:failure.generic");
  });

  /**
   * **`common:failure.*` 里已经写过精确文案的码，队列面板必须用上那句话。**
   *
   * 此前抓取码之外一律降级成 `failure.generic`（「任务未能完成」）。于是
   * `DISTILL_JOB: QUOTA_EXHAUSTED` 这类**可行动**的码在面板上完全不可见：`common.json`
   * 为它写了一整句「去哪儿看余量、补足后重试、或改用本机模型」，而用户只能把鼠标
   * 悬到那一行上才看得到原始错误串。
   */
  it("外层码套内层码时，取的是内层那个有文案的码", () => {
    const t = (key: string) => `T:${key}`;
    // 后端的真实形态：`format!("DISTILL_JOB: {}", err.code())`。
    expect(taskFailureText("DISTILL_JOB: QUOTA_EXHAUSTED", t)).toBe(
      "T:common:failure.QUOTA_EXHAUSTED",
    );
    expect(taskFailureText("QUOTA_EXHAUSTED", t)).toBe("T:common:failure.QUOTA_EXHAUSTED");
    // 外层码自己没有文案，不许被当成答案。
    expect(taskFailureText("DISTILL_JOB", t)).toBe("T:common:failure.generic");
    // 抓取码仍然优先（它的措辞更贴那条路径）。
    expect(taskFailureText("STRICT_LOCAL_BLOCKED", t)).toBe("T:fetchError.strictLocal");
  });

  /**
   * **`INVALID_INPUT` 不许说「你的输入不对」**（2026-08-31，ADR-103）。
   *
   * 这两条 LLM 通道上，`JobError::InvalidInput` 的生产者**逐个数得清**，没有一个是
   * 用户的正文：
   * - `cloud_llm.rs` 服务端码 `invalid_input`——它自己又有两个来源：服务端的入参校验
   *   （请求是**我们**构造的：capability / service_class / role / messages 条数与长度，
   *   用户碰不到任何一格），以及「候选链上每一台上游都拒绝了这次请求」
   *   （上游 400/404/422/413 在服务端统一判成 `invalid_input`）；
   * - `cloud_llm.rs` / `llm_http.rs` 的 `messages.is_empty()`——「本 Provider 不支持
   *   该 Job 类型」，是我们的构造 bug；
   * - `cloud_llm.rs` 的 `idempotency_key_reused`——同上，我们这一侧的 bug。
   *
   * 所以旧文案「当前输入不适用于此操作」在**每一次**出现时都是错的：它把用户支去改一篇
   * 没有问题的笔记，而真正该做的是缩短内容或找管理员。
   */
  it("「输入非法」不许把责任推给用户的正文", () => {
    // 对话表面自 ADR-111 ④（批 E2）起共用同一句 common 文案，只需守注册表这一份。
    expect(zhCommon.failure.INVALID_INPUT).not.toMatch(/输入(不|无法|非法)|你的内容|内容不(对|合法)/);
    expect(zhCommon.failure.INVALID_INPUT).toMatch(/管理员/);
    expect(enCommon.failure.INVALID_INPUT).not.toMatch(/input is (not |in)valid|your (input|content) is/i);
    expect(enCommon.failure.INVALID_INPUT).toMatch(/administrator/i);
  });

  /**
   * **真机回归（2026-08-31，ADR-102）**：「合成主题页」失败，队列面板显示的是
   * 「AI 引擎返回错误，请重试」——因为服务端选中的推理模型只发 `thinking` 帧、正文帧
   * 0 个，而客户端当时把这一档压进 `Upstream`，那句可行动的指引写在一个不进 DTO 的
   * 字段里，一个字都没到过界面。
   *
   * 判据钉在**这一行原样的后端串**上，而不是钉在码本身：这条路径的实际形态是
   * `format!("DISTILL_JOB: {}", err.code())`，只断言码等于绕开了那一层拼接。
   */
  it("模型只吐推理、正文为空时，队列面板说的不再是「上游返回错误」", () => {
    const t = (key: string) => `T:${key}`;
    expect(taskFailureText("DISTILL_JOB: EMPTY_AFTER_THINKING", t)).toBe(
      "T:common:failure.EMPTY_AFTER_THINKING",
    );
    // 它与 `UPSTREAM` 必须是两句不同的话——合并回去，本条缺陷当场复发。
    expect(zhCommon.failure.EMPTY_AFTER_THINKING).not.toBe(zhCommon.failure.UPSTREAM);
    expect(enCommon.failure.EMPTY_AFTER_THINKING).not.toBe(enCommon.failure.UPSTREAM);
    // 两条**开不出来的动作**都不许出现在文案里：客户端侧的输出预算是编译期常量
    // （界面上没有那个旋钮），服务端托管形态下模型也由服务端挑（用户换不了）。
    // 对话表面自 ADR-111 ④（批 E2）起共用同一句 common 文案，不再单独断言第二份。
    expect(zhCommon.failure.EMPTY_AFTER_THINKING).not.toMatch(/预算|额度上限|换.{0,2}模型/);
    expect(zhCommon.failure.EMPTY_AFTER_THINKING).toMatch(/重试/);
  });

  /**
   * 存在性判据必须问 i18n 本身，不许在这个文件里另抄一份码表。
   *
   * 抄一份的话，下次谁给某个码补了 `common:failure.*` 文案、队列面板仍旧不认它——
   * 而那正是这次要修的那个缺陷的形状。
   */
  it("没有精确文案的码照旧走中性兜底（不许把裸码糊到界面上）", () => {
    const t = (key: string) => `T:${key}`;
    for (const code of ["DECODE_FAILED", "TRANSLATE_NO_TRANSCRIPT", "DIARIZE_MODELS_MISSING"]) {
      expect(taskFailureText(`${code}: 细节`, t)).toBe("T:common:failure.generic");
    }
  });
});

/**
 * 「额度不足」的措辞必须与**起跑线**语义一致（D7′ / PRD-13-040 ②）。
 *
 * 目标态下这个码只在任务**开始之前**出现：服务端不再因为中途触顶把整件事判失败
 * （那条改动在服务端批），于是「重试不会成功」这句话不再成立——预付 / 加量场景里，
 * 补足额度之后重试**就会**成功。写着"重试不会成功"的提示会让一个花钱就能解决的问题
 * 看起来像一堵墙。
 */
describe("额度文案说的是起跑线，不是半路中断", () => {
  it("不再断言「重试不会成功」", () => {
    for (const [name, text] of [
      ["zh-CN common", zhCommon.failure.QUOTA_EXHAUSTED],
      ["en-US common", enCommon.failure.QUOTA_EXHAUSTED],
    ] as const) {
      expect(text, `${name} 缺文案`).toBeTruthy();
      expect(text, `${name}: 补足额度后重试是会成功的`).not.toMatch(
        /重试不会成功|retrying will not help/i,
      );
    }
  });

  it("说清「这次还没开始」，且给得出下一步", () => {
    // 起跑线语义的全部价值：用户知道没有半成品、没有被扣，且知道补完额度就能继续。
    expect(zhCommon.failure.QUOTA_EXHAUSTED).toMatch(/没有开始|未开始/);
    expect(enCommon.failure.QUOTA_EXHAUSTED).toMatch(/never started/i);
    expect(zhCommon.failure.QUOTA_EXHAUSTED, "要给得出下一步：补额度 / 换本机模型").toMatch(
      /本机模型/,
    );
  });
});
