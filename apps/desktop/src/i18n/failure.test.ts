import { describe, expect, it } from "vitest";
import zh from "./locales/zh-CN/common.json";
import en from "./locales/en-US/common.json";
import { billingGuidance, failureGuidance, failureMessage } from "./failure";
import { i18n } from "./i18n";
import { jobErrorCodesFromBindings } from "./job-error-codes-test-fixture";

/**
 * 码的全集来自生成物 `bindings.ts`（`job-error-codes-test-fixture.ts`），不再手抄：
 * 手抄表在 2026-08 被发现漏了 4 个码而长期全绿——`satisfies` 只守得住「后端删了」这一半。
 *
 * 「每个码在它能到达的通道上都有中英文案」这条断言已搬到 `job-error-code-channels.test.ts`
 * （它按通道逐码判定，翻译通道只是其中一条）；这里只留翻译通道**修复动作**的守护。
 */
const ALL_CODES = jobErrorCodesFromBindings();

describe("Job 失败码修复动作（TDD-15 §4.3 错误码即文案 key）", () => {
  it("登记了修复动作的码，其按钮文案在两份资源包里都在", () => {
    let registered = 0;
    for (const code of ALL_CODES) {
      const guidance = failureGuidance(code);
      if (!guidance) continue;
      registered += 1;
      // labelKey 形如 `common:failure.action.open_models`——按 `.` 逐层取，
      // 写错一层就会拿到 undefined（界面上表现为按钮显示裸 key）。
      const path = guidance.labelKey.replace(/^common:/, "").split(".");
      for (const [name, bundle] of [
        ["zh", zh],
        ["en", en],
      ] as const) {
        const label = path.reduce<unknown>(
          (node, seg) => (node as Record<string, unknown> | undefined)?.[seg],
          bundle,
        );
        expect(label, `${name} 缺 ${guidance.labelKey}（${code}）`).toBeTruthy();
      }
    }
    // 循环体真的跑到过：修复动作表若被清空，上面的断言会一条都不执行而绿。
    expect(registered).toBeGreaterThan(0);
  });

  /**
   * 「需要登录」必须带可执行动作，且门是账户页。
   *
   * 它此前只能借 `UPSTREAM` 表达，于是界面说的是「AI 引擎返回错误，请重试」——
   * 最有可执行动作的一类失败被显示成最没动作的那句，而重试永远不会成功。
   */
  it("SIGN_IN_REQUIRED 去账户页登录，而不是无动作的通用上游错误", async () => {
    const session = failureGuidance("SIGN_IN_REQUIRED");
    expect(session?.route).toBe("settings://account");
    expect(session?.labelKey).toBe("common:failure.action.sign_in");
    expect(failureGuidance("UPSTREAM")).toBeNull();
    const zhFailure = zh.failure as Record<string, unknown>;
    const enFailure = en.failure as Record<string, unknown>;
    expect(zhFailure.SIGN_IN_REQUIRED).toMatch(/账户/);
    expect(enFailure.SIGN_IN_REQUIRED).toMatch(/Account/);
  });

  /**
   * H1（判据 T1.4）：成因句开出「切回本机模型」这一本机动作的码，卡片上必须有那扇门。
   * 判据直接读两份资源包：哪句文案提到了本机模型，哪个码就必须登记修复动作，且落在
   * 「AI 算力与模型」——此前 `SERVER_SIDE_FAILURE` 的文案说「或先切回本机模型」，卡片上
   * 却只有一颗永远失败的「重新生成」。
   */
  it("文案里开出「切回本机模型」的码，都带指向算力页的修复动作", () => {
    let checked = 0;
    for (const code of ALL_CODES) {
      const zhCopy = (zh.failure as Record<string, unknown>)[code];
      const enCopy = (en.failure as Record<string, unknown>)[code];
      const offersLocal =
        (typeof zhCopy === "string" && zhCopy.includes("本机模型")) ||
        (typeof enCopy === "string" && /on-device model|local model/i.test(enCopy));
      if (!offersLocal) continue;
      checked += 1;
      const guidance = failureGuidance(code);
      expect(guidance, `${code} 的文案指了本机模型，卡片却没有门`).not.toBeNull();
      expect(guidance?.route, code).toMatch(/^settings:\/\/(ai_services|account)/);
    }
    expect(checked).toBeGreaterThanOrEqual(3);
    for (const code of ["SERVER_SIDE_FAILURE", "SERVER_MISCONFIGURED", "ROUTE_LANE_EMPTY"]) {
      expect(failureGuidance(code)?.route, code).toBe("settings://ai_services");
    }
    // 没有开出本机动作的服务端码仍然刻意无门（ADR-102 ④）。
    expect(failureGuidance("EMPTY_AFTER_THINKING")).toBeNull();
  });

  it("账户被冻结（纵切 3 C3）去账户页看说明，且没有「升级」——冻结时结账同样被拒", () => {
    const frozen = failureGuidance("ACCOUNT_FROZEN");
    expect(frozen?.route).toBe("settings://account/billing");
    expect(frozen?.labelKey).toBe("common:failure.action.open_account");
    expect(billingGuidance("ACCOUNT_FROZEN")).toBeNull();
    const zhFailure = zh.failure as Record<string, unknown>;
    const enFailure = en.failure as Record<string, unknown>;
    expect(zhFailure.ACCOUNT_FROZEN).toMatch(/联系支持/);
    expect(enFailure.ACCOUNT_FROZEN).toMatch(/contact support/i);
    expect(zhFailure.ACCOUNT_FROZEN).not.toBe(zhFailure.QUOTA_EXHAUSTED);
  });

  it("撞上自设的本月预付上限（纵切 3 C4）去调整上限，且没有「升级」——那是用户自己写下的预算", () => {
    const capped = failureGuidance("SPEND_CAP_REACHED");
    expect(capped?.route).toBe("settings://account/spend_cap");
    expect(capped?.labelKey).toBe("common:failure.action.open_spend_cap");
    expect(billingGuidance("SPEND_CAP_REACHED")).toBeNull();
    const zhFailure = zh.failure as Record<string, unknown>;
    const enFailure = en.failure as Record<string, unknown>;
    expect(zhFailure.SPEND_CAP_REACHED).toMatch(/本月预付额度上限/);
    expect(enFailure.SPEND_CAP_REACHED).toMatch(/spending cap/i);
    expect(zhFailure.SPEND_CAP_REACHED).not.toBe(zhFailure.QUOTA_EXHAUSTED);
    expect((zh.failure.action as Record<string, unknown>).open_spend_cap).toBeTypeOf("string");
    expect((en.failure.action as Record<string, unknown>).open_spend_cap).toBeTypeOf("string");
  });

  it("领不到免费额度（纵切 3 C5）去账户页的免费额度说明，且没有「升级」——要验证的人只差一次零扣费验证", () => {
    const withheld = failureGuidance("FREE_QUOTA_WITHHELD");
    expect(withheld?.route).toBe("settings://account/free_quota");
    expect(withheld?.labelKey).toBe("common:failure.action.open_free_quota");
    expect(billingGuidance("FREE_QUOTA_WITHHELD")).toBeNull();
    const zhFailure = zh.failure as Record<string, unknown>;
    const enFailure = en.failure as Record<string, unknown>;
    expect(zhFailure.FREE_QUOTA_WITHHELD).toMatch(/免费额度/);
    expect(enFailure.FREE_QUOTA_WITHHELD).toMatch(/free quota/i);
    expect(zhFailure.FREE_QUOTA_WITHHELD).not.toBe(zhFailure.QUOTA_EXHAUSTED);
    expect((zh.failure.action as Record<string, unknown>).open_free_quota).toBeTypeOf("string");
    expect((en.failure.action as Record<string, unknown>).open_free_quota).toBeTypeOf("string");
  });

  it("Provider 未装配去 AI 服务页", () => {
    expect(failureGuidance("PROVIDER_NOT_CONFIGURED")?.route).toBe("settings://ai_services");
  });

  // ── 原 chat-failure.ts 那张第二深链表的判决，随 ADR-111 ④ 合并搬入 ────────────

  it("额度用尽深链到账户页，不是模型页——两张表时代只有 chat 知道这条修正", () => {
    // 修复动作是「看余量/重置时间/找管理员加量」，跟本机装了哪些模型毫无关系；
    // 指去模型页 = 让用户白跑一趟。单表之后翻译弹窗第一次也拿到了这条。
    expect(failureGuidance("QUOTA_EXHAUSTED")?.route).toBe("settings://account");
    expect(failureGuidance("MODEL_MISSING")?.route).toBe("settings://ai_services");
    expect(failureGuidance("STRICT_LOCAL_BLOCKED")?.route).toBe("settings://privacy");
    expect(failureGuidance("RESOURCE_EXHAUSTED")?.route).toBe("settings://ai_services");
  });

  it("刻意缺席的码保持缺席：文案没开出本机动作，深链只会把用户送错地方", () => {
    // 2026-09-01 修订（H1 / ADR-102 ④）：`ROUTE_LANE_EMPTY` / `SERVER_SIDE_FAILURE` /
    // `SERVER_MISCONFIGURED` 从这份名单移出——它们的成因句自己写着「切回本机模型」，
    // 缺席等于文案指着一扇卡片上没有的门。上一条测试按那条判据反向守护它们。
    for (const code of [
      "EMPTY_AFTER_THINKING",
      // 「重试」= 新 Job、新幂等键、按第二次计费——不是修复动作。
      "ALREADY_PROCESSED",
    ]) {
      expect(failureGuidance(code), `${code} 不该有深链`).toBeNull();
    }
  });

  it("未知码回退 generic 兜底，且兜底在两份资源包里都在（否则回落本身就是裸 key）", async () => {
    for (const lng of ["zh-CN", "en-US"] as const) {
      await i18n.changeLanguage(lng);
      expect(failureMessage("WEIRD_NEW_CODE")).toBe(i18n.t("common:failure.generic"));
      expect(i18n.t("common:failure.generic")).not.toContain("failure.generic");
    }
    await i18n.changeLanguage("zh-CN");
  });
});
