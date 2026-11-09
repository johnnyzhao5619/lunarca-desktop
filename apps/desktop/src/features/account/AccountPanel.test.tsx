/**
 * 账户面板的失败呈现守护。
 *
 * # 为什么这个文件迟到了
 *
 * 验收（2026-08-08）时的事实：`features/account/` 下**一个测试都没有**，而面板本体
 * 六百余行、登录 hook 近两百行。同一天真机截图上那三个 P0——红色横幅永久钉在页顶、
 * 引导页登录失败毫无反应、目录三行自相矛盾——在 706 个前端用例全绿的情况下出厂。
 * 门禁绿而产品坏，差的不是纪律，是这一层压根没有被任何断言覆盖。
 *
 * 本文件只钉**失败路径的呈现**：成功路径出问题用户会立刻发现并报告，失败路径出问题
 * 用户只会觉得"这软件怪怪的"然后走人。三条用例逐一对应上面三个缺陷。
 *
 * 渲染走仓库既有的 `renderToStaticMarkup` 夹具（不引 DOM 测试库，理由见夹具头注释）：
 * effect 不跑，故断言的是**首帧结构**——而这三个缺陷恰好全部呈现在首帧。
 */

import { createElement } from "react";
import { describe, expect, it } from "vitest";

import cloudEn from "../../i18n/locales/en-US/cloud.json";
import cloudZh from "../../i18n/locales/zh-CN/cloud.json";
import {
  choiceAfterSwitchingFrom,
  logoutConfirmation,
  SignedInView,
  switchServerConfirmation,
  UsageSection,
} from "./AccountPanel";
import { errorAction, errorMessage, errorText } from "../../lib/cloud-error";
import { formatInstant } from "../../lib/format-datetime";
import { renderWithSettingsHosts, translateForTest } from "../settings/settings-test-fixture";
import type {
  CloudError,
  DeviceDto,
  EntitlementsDto,
  UsageLedgerDto,
} from "../../ipc/cloud-bindings";

const translateCloud = (key: string, options?: Record<string, unknown>) =>
  translateForTest(`cloud:${key}`, options);

describe("账户 · 失败呈现", () => {
  /**
   * 失败就地显示原因：译文而不是裸 key——否则组件与用例会同时回落成同一个 key，断言退化成
   * 同义反复，缺文案时照样绿。
   */
  it("登录失败的原因渲染成译文", () => {
    const message = errorText({ code: "not_an_address" }, translateCloud);
    expect(message).not.toBe("error.not_an_address");
    expect(message).toBe(translateForTest("cloud:error.not_an_address"));
    expect(errorText({ code: "link_rejected", reason: { reason: "bad_server" } }, translateCloud)).toBe(
      translateForTest("cloud:error.link_rejected.bad_server"),
    );
    // 自签证书连不上：下一步说「用邮箱和密码登录即可」（ADR-158），而不是「去装根证书」。
    expect(
      errorAction(
        { code: "network", host: "10.0.0.62:8443", detail: "", kind: "tls" },
        translateCloud,
      ),
    ).toBe(translateForTest("cloud:error.network_action.tls"));
  });

  /**
   * 2026-10-13 真机：登录页连不上时，原因句里自带一句「请确认…」，下面的分档指引又说一遍。
   * 原因只说原因（分档），下一步只在 `errorAction`；一行位置的地方才把两句连起来。
   */
  it("连不上：原因与下一步各说一次", () => {
    const error = { code: "network", host: "10.0.0.62:8443", detail: "", kind: "unreachable" } as const;
    const text = errorText(error, translateCloud);
    const action = errorAction(error, translateCloud);
    expect(text).toBe(translateForTest("cloud:error.network_kind.unreachable", { host: "10.0.0.62:8443" }));
    expect(action).toBe(translateForTest("cloud:error.network_action.unreachable"));
    expect(text).not.toContain(action ?? "");
    expect(errorMessage(error, translateCloud)).toBe(
      translateForTest("cloud:error.with_action", { text, action }),
    );
    // 本地网络权限这条线索必须出现在下一步里：调试包重编译后它会失效，表现正是这一档。
    expect(action).toMatch(/本地网络|Local Network/);
  });

  /**
   * `CloudError` 的每个 code 都必须有中英文案。缺 key 时 i18next 回落成裸串，
   * 用户看到的是 `error.invalid_endpoint` 这种东西——而这恰恰发生在他已经出错的时刻。
   * code 是 Rust 侧的稳定契约，新增一个而忘了配文案，本用例即红。
   */
  it("每个云错误码在中英资源包里都有文案", () => {
    const codes = [
      "not_logged_in",
      "needs_relogin",
      "no_pending_login",
      "login_expired",
      "strict_local_blocked",
      "invalid_endpoint",
      "server",
      "rate_limited",
      "already_signed_in",
      "not_an_address",
      "no_server_at_address",
      "not_an_email",
      "server_identity_rejected",
      "client_too_old",
      "server_too_old",
      "no_invite",
      "billing_disabled",
      "plan_unavailable",
      "no_billing_account",
      "billing_unavailable",
      "last_org_owner",
      "export_write_failed",
      "account_frozen",
      "free_quota_withheld",
      "verification_not_required",
      "device_limit_expired",
      "email_domain_not_allowed",
      "keychain",
      "internal",
    ] as const;
    for (const code of codes) {
      expect(cloudZh.error[code], `zh-CN 缺 error.${code}`).toBeTypeOf("string");
      expect(cloudEn.error[code], `en-US 缺 error.${code}`).toBeTypeOf("string");
    }
    // `network` 按原因分档：每一档都要有原因与下一步两句。
    const kinds = ["dns", "refused", "timeout", "tls", "unreachable", "other"] as const;
    for (const kind of kinds) {
      for (const bundle of [cloudZh, cloudEn]) {
        expect(bundle.error.network_kind[kind], `缺 network_kind.${kind}`).toBeTypeOf("string");
        expect(bundle.error.network_action[kind], `缺 network_action.${kind}`).toBeTypeOf("string");
      }
    }
  });

  /**
   * 目录区块此前是三行，其中两行在 Verified 态说同一句话、第三行永远与它们矛盾
   * （「已验签 v3 / 尚未刷新」——Rust 侧不持久化 `last_refresh_at` 所致，那一半已修）。
   * 呈现这一半的回归钉子：被合并掉的两个 key 不得复活，否则冗余会原样长回来。
   */
  it("目录区块不再保留与状态行重复的「目录版本」文案", () => {
    for (const bundle of [cloudZh, cloudEn]) {
      const catalog = bundle.catalog as Record<string, unknown>;
      expect(catalog.current_label, "「当前目录」是合并后的唯一标签").toBeTypeOf("string");
      expect(catalog.version_value, "与状态行重复，已合并").toBeUndefined();
      expect(catalog.version_label, "与状态行重复，已合并").toBeUndefined();
      expect(catalog.version_bundled, "与状态行重复，已合并").toBeUndefined();
      // 「被拒」并进「上次刷新」那一行——它描述的是上一次去拿的结果，不是当前生效的目录。
      expect(catalog.refreshed_rejected, "被拒原因跟着刷新时刻走").toBeTypeOf("string");
      expect(catalog.state_rejected, "不再单独成行").toBeUndefined();
    }
  });

  /**
   * `rejected` 说的是"上一次去拿被拒了"，而此刻生效的仍是最近一份有效副本
   * （TDD-14 §3.1 fallback）。把它写成「当前目录：已拒绝」是**假的**——用户会以为
   * 自己此刻没有可用目录，而他有。这是本次修复过程中一度写错、由自审抓回来的一条，
   * 故留一枚钉子：两个字段各答一个问题，串在一起答就必然有一个答错。
   */
  it("被拒时「当前目录」仍如实报告生效中的那一份", () => {
    const verifiedCopy = cloudZh.catalog.state_verified;
    const bundledCopy = cloudZh.catalog.state_bundled;
    expect(verifiedCopy).toBeTypeOf("string");
    expect(bundledCopy).toBeTypeOf("string");
    // 「当前目录」的取值集合只有这两种；「已拒绝」不属于它。
    expect(verifiedCopy).not.toContain(cloudZh.catalog.refreshed_rejected);
  });

  /** 横幅可关闭这件事有文案支撑（关闭按钮的无障碍名不得是硬编码）。 */
  it("错误横幅的关闭动作有中英文案", () => {
    expect(cloudZh.actions.dismiss).toBeTypeOf("string");
    expect(cloudEn.actions.dismiss).toBeTypeOf("string");
  });

  // ---- 用量与额度（纵切 2 收口 ④）----

  function renderUsage(
    ledgers: UsageLedgerDto[] | null,
    error?: CloudError,
    entitlements: EntitlementsDto | null = null,
  ): string {
    return renderWithSettingsHosts(
      createElement(UsageSection, { ledgers, entitlements, error }),
    );
  }

  function ents(...pairs: [string, unknown][]): EntitlementsDto {
    return {
      tier: "free",
      entitlements: pairs.map(([key, value]) => ({
        key,
        value_json: JSON.stringify(value),
      })),
      lease_verified: true,
      lease_expires_at: null,
      lease_error: null, capabilities: null, billing_standing: { state: "good" }, free_quota: { state: "not_applicable" },
    };
  }

  const subscription: UsageLedgerDto = {
    kind: "subscription",
    period: "monthly",
    dimensions: [
      { dimension: "tokens_in", limit: 1000, used: 120, alert: "none" },
      { dimension: "egress_bytes", limit: null, used: 4096, alert: "none" },
    ],
  };

  /** 可用的 AI 能力（R4b）：Rust 解析好的清单渲染成一行人话，原始那条权益不再重复露出。 */
  it("AI 能力一行：部分开放逐项列名，全开说全部，全关说都在本机", () => {
    const raw = ["inference.capabilities", ["chat", "transcribe"]] as [string, unknown];
    const partial = renderUsage([subscription], undefined, {
      ...ents(raw),
      capabilities: ["transcribe", "chat"],
    });
    expect(partial).toContain(translateForTest("cloud:entitlements.capabilities"));
    expect(partial).toContain(
      [translateForTest("cloud:entitlements.capability.chat"), translateForTest("cloud:entitlements.capability.transcribe")]
        .join(translateForTest("cloud:entitlements.capability_separator")),
    );
    expect(partial).not.toContain("inference.capabilities");
    const all = ["chat", "translate", "summarize", "refine", "embed", "transcribe"];
    expect(renderUsage([subscription], undefined, { ...ents(), capabilities: all })).toContain(
      translateForTest("cloud:entitlements.capabilities_all"),
    );
    expect(renderUsage([subscription], undefined, { ...ents(), capabilities: [] })).toContain(
      translateForTest("cloud:entitlements.capabilities_none"),
    );
  });

  /**
   * **取不到 ≠ 没有用量。**
   *
   * 此前 `/v1/usage` 失败被整个吞掉，区块把「还没取到」渲染成「尚未用过服务端算力」
   * ——一句关于事实的断言，而事实是我们根本不知道。私有化服务端只实现推理面、不实现
   * `/v1/usage` 是正常部署形态，那种机器上这句话每次都会出现。设备与权益两块早就是
   * 「就地报错」，用量是同一个缺陷**漏改的第三处**。
   */
  it("用量取不到时如实报错，不谎称「尚未用过服务端算力」", () => {
    const html = renderUsage([], { code: "server", status: 404, error_code: "not_found" });
    const empty = translateForTest("cloud:usage.empty");
    expect(empty).not.toBe("cloud:usage.empty");
    expect(html).not.toContain(empty);
    // 文案带 HTTP 状态码（ADR-116 ④：502 与 404 的下一步完全不同，状态码就是分档依据）。
    const reason = translateForTest("cloud:error.server", { status: 404 });
    expect(reason).not.toBe("cloud:error.server");
    expect(html).toContain(reason);
  });

  it("真的取到空列表时才说「尚未用过服务端算力」", () => {
    expect(renderUsage([])).toContain(translateForTest("cloud:usage.empty"));
  });

  /**
   * **每本账各占一段，周期写在各自账本下。**
   *
   * 订阅额度按周期重置、预付余额不重置，合并成一个数既回答不了「这个月还剩多少」
   * 也回答不了「我买的流量包用掉多少」（PRD-13-038）。周期挂在区块底部同理会被
   * 读成对两本账都成立。
   */
  it("每本账各带自己的标题与重置周期", () => {
    const prepaid: UsageLedgerDto = {
      kind: "prepaid",
      period: "none",
      dimensions: [{ dimension: "tokens_in", limit: 5000, used: 0, alert: "none" }],
    };
    const html = renderUsage([subscription, prepaid]);
    expect(html).toContain(translateForTest("cloud:usage.ledger_subscription"));
    expect(html).toContain(translateForTest("cloud:usage.ledger_prepaid"));
    // 两个周期同时在场 = 它们各自挂在自己那本账下，而不是共用一句。
    expect(html).toContain(translateForTest("cloud:usage.period_monthly"));
    expect(html).toContain(translateForTest("cloud:usage.period_none"));
  });

  /**
   * 「不限」不画进度条，也不显示成一个假的上限。把「随便用」渲染成「快用完了」
   * 是最坏的一种误导。
   */
  it("limit 为 null 时渲染「不限」而不是一个数字上限", () => {
    const html = renderUsage([subscription]);
    // 该维度是 `egress_bytes` —— 字节量按 IEC 单位渲染，不是裸数字（见 `formatBytes`）。
    expect(html).toContain(
      translateForTest("cloud:usage.amount_unlimited", { used: "4\u00a0KiB" }),
    );
  });

  /**
   * 字节维度必须带单位。
   *
   * 真机上出站额度是 `5,368,709,120`——裸数字既读不出量级，也无法与运营口中的额度
   * 对上；而次数/token 维度必须保持整数千分位，不能被换算成「1.2 KiB」这种荒唐值。
   */
  it("字节维度按 IEC 单位渲染，计数维度保持千分位整数", () => {
    const html = renderUsage([
      {
        kind: "subscription",
        period: "monthly",
        dimensions: [
          { dimension: "egress_bytes", limit: 5_368_709_120, used: 1_024_591, alert: "none" },
          { dimension: "requests", limit: 10_000, used: 1234, alert: "none" },
        ],
      },
    ]);
    expect(html).toContain(
      translateForTest("cloud:usage.amount", { used: "1,000\u00a0KiB", limit: "5\u00a0GiB" }),
    );
    expect(html).toContain(
      translateForTest("cloud:usage.amount", { used: "1,234", limit: "10,000" }),
    );
  });

  /** 账本标题的中英文案齐备——缺 key 时用户看到的是 `usage.ledger_subscription`。 */
  it("两本账的标题在中英资源包里都有文案", () => {
    for (const bundle of [cloudZh, cloudEn]) {
      const usage = bundle.usage as Record<string, unknown>;
      expect(usage.ledger_subscription).toBeTypeOf("string");
      expect(usage.ledger_prepaid, "预付账本上线时不该再补文案").toBeTypeOf("string");
    }
  });

  /**
   * **五维 Meter 与四种周期的文案必须齐备，且这一条只能由清单来钉。**
   *
   * 维度名与周期名都是**服务端下发的字符串**（ADR-034 五维 Meter；周期来自配额策略），
   * 渲染时走 `t(key, { defaultValue: 原字符串 })`——缺文案不会报错、不会红，只会把
   * `storage_byte_hours` 原样画在账单上。既有用例只碰过 `egress_bytes` / `requests` /
   * `monthly` / `none` 四个，另外四个（`tokens_in`、`tokens_out`、`asr_seconds`、
   * `daily`/`weekly`）在两个资源包里**从未被任何断言看过**。
   *
   * 清单逐字取自服务端 `MeterEvent` 的五维 + 请求数，与配额策略的四种周期。
   */
  it("五维 Meter 与四种重置周期在中英资源包里都有文案", () => {
    const dimensions = [
      "tokens_in",
      "tokens_out",
      "asr_seconds",
      "storage_byte_hours",
      "egress_bytes",
      "requests",
      // 预付账本那一维：Rust 把线上的微 credit 换算成 credit 并改名（纵切 3 C4）。
      "credits",
    ];
    const periods = ["none", "daily", "weekly", "monthly"];
    for (const bundle of [cloudZh, cloudEn]) {
      const usage = bundle.usage as Record<string, unknown>;
      const dimensionCopy = usage.dimension as Record<string, unknown>;
      for (const name of dimensions) {
        expect(
          dimensionCopy[name],
          `缺 usage.dimension.${name} —— 账单上会直接出现 ${name} 这个内部标识`,
        ).toBeTypeOf("string");
      }
      for (const period of periods) {
        expect(
          usage[`period_${period}`],
          `缺 usage.period_${period} —— 那一行会写成 ${period}`,
        ).toBeTypeOf("string");
      }
    }
  });

  /**
   * **文案齐备还不够：渲染出来的必须是译文，不是那个兜底的原字符串。**
   *
   * 上一条钉资源包，这一条钉那条 `defaultValue` 兜底没有把译文顶掉——两者一起才排除
   * 「资源包里有、组件却没读到」这种命名空间/加载顺序问题（它在首帧就现形，恰好是本
   * 夹具能看到的）。
   */
  it("维度名渲染成译文而不是服务端下发的内部标识", () => {
    const html = renderUsage([
      {
        kind: "subscription",
        period: "monthly",
        dimensions: [{ dimension: "asr_seconds", limit: null, used: 804, alert: "none" }],
      },
    ]);
    const label = translateForTest("cloud:usage.dimension.asr_seconds");
    expect(label).not.toBe("cloud:usage.dimension.asr_seconds");
    expect(label).not.toBe("asr_seconds");
    expect(html).toContain(label);
    expect(html, "内部标识漏进了界面").not.toContain(">asr_seconds<");
  });

  // ---- 权益与账本同处一段（ADR-121 ②③⑤⑥）----

  /**
   * **四条权益此前 4/4 都在露 key。**
   *
   * `t('entitlement.<key>', { defaultValue: item.key })` 是一条正确的兜底裁决，但
   * `cloud.json` 里从来没有 `entitlement` 这个命名空间——于是服务端 free 档下发的
   * `devices.max` / `catalog.refresh` / `cloud.inference` / `sync.enabled` 原样画在
   * 屏幕上。既有的 i18n 存在性校验查不到它：那个 key 只在运行时拼出来。
   *
   * 清单逐字取自服务端 `EntitlementSet::free()`。
   */
  it("服务端 free 档的四条权益都有登记文案，一条都不露 key", () => {
    const keys = ["devices.max", "catalog.refresh", "cloud.inference", "sync.enabled"];
    const html = renderUsage(
      [],
      undefined,
      ents(...keys.map((k) => [k, true] as [string, unknown])),
    );
    for (const key of keys) {
      const label = translateForTest(`cloud:entitlement.${key.replace(/\./g, "_")}`);
      expect(label, `缺 entitlement.${key} 的登记文案`).not.toContain("cloud:");
      expect(html).toContain(label);
      expect(html, `${key} 作为裸标识符漏进了界面`).not.toContain(`>${key}<`);
    }
  });

  /** 未登记的 key 走中性标签，标识符降为副信息——不占标签位（ADR-121 ③）。 */
  it("未登记的权益 key 不占标签位，只作副信息出现", () => {
    const html = renderUsage([], undefined, ents(["future.capability", 3]));
    const neutral = translateForTest("cloud:entitlements.other");
    expect(neutral).not.toContain("cloud:");
    expect(html).toContain(neutral);
    // 标识符仍然看得见（宁可露出也不吞掉），但它在副信息行里，不在标题里。
    expect(html).toContain("future.capability");
    expect(html).toContain(
      `<p class="luna-settings-panel__row-desc">future.capability</p>`,
    );
    expect(html).not.toContain(
      `<span class="luna-settings-panel__row-title">future.capability</span>`,
    );
  });

  /**
   * 「订阅包含」不再是独立一段：它与用量是同一件事的两面，分两段读者要自己对照。
   * 判据是段标题**只有一个**，而不是"页面里找不到权益"——权益照常在，只是同段。
   */
  it("权益与账本同处一段，段内没有第二个段标题", () => {
    const html = renderUsage([subscription], undefined, ents(["cloud.inference", true]));
    const titles = html.match(/luna-settings-panel__section-title/g) ?? [];
    expect(titles).toHaveLength(1);
    expect(html).toContain(translateForTest("cloud:entitlement.cloud_inference"));
  });

  /**
   * 重置信息在**账本标题**上，不逐行重复。
   *
   * 一个 `period` 管这本账的全部维度；搬到每一行就是同一句话说六遍（硬约束 4）。
   * 它此前是账本底部一句没有主语的孤句——真机上读作「不重置」三个字孤零零挂在表下。
   */
  it("重置信息只在账本标题上出现一次", () => {
    const html = renderUsage([
      {
        kind: "subscription",
        period: "monthly",
        dimensions: [
          { dimension: "tokens_in", limit: 1000, used: 10, alert: "none" },
          { dimension: "tokens_out", limit: 1000, used: 20, alert: "none" },
          { dimension: "requests", limit: 100, used: 3, alert: "none" },
        ],
      },
    ]);
    const period = translateForTest("cloud:usage.period_monthly");
    expect(html.split(period)).toHaveLength(2);
    expect(html).toContain(
      translateForTest("cloud:usage.ledger_heading", {
        ledger: translateForTest("cloud:usage.ledger_subscription"),
        period,
      }),
    );
  });

  /** 认不出的周期就不提重置——原样写上去既是裸标识符，也等于替服务端宣布我们不知道的事。 */
  it("认不出的周期不把标识符写到标题上", () => {
    const html = renderUsage([
      {
        kind: "subscription",
        period: "fortnightly",
        dimensions: [{ dimension: "tokens_in", limit: 1000, used: 10, alert: "none" }],
      },
    ]);
    expect(html).not.toContain("fortnightly");
    expect(html).toContain(translateForTest("cloud:usage.ledger_subscription"));
  });

  /**
   * 有上限就有条，且**百分比不写成可见文字**——条已经表达了它，`<progress value max>`
   * 天然把百分比交给屏幕阅读器（ADR-121 ⑤）。
   */
  it("有上限的维度画进度条，且不把百分比再写成一遍文字", () => {
    const html = renderUsage([
      {
        kind: "subscription",
        period: "monthly",
        dimensions: [{ dimension: "tokens_in", limit: 1000, used: 250, alert: "none" }],
      },
    ]);
    expect(html).toContain('<progress class="luna-progress" value="250" max="1000"');
    expect(html, "百分比被写成了第二遍可见文字").not.toMatch(/>\s*25\s*%/);
  });

  /** 不限的那一维不画条：把「随便用」画成「快没了」是最坏的一种误导。 */
  it("不限的维度不画进度条", () => {
    const html = renderUsage([
      {
        kind: "subscription",
        period: "monthly",
        dimensions: [{ dimension: "egress_bytes", limit: null, used: 4096, alert: "none" }],
      },
    ]);
    expect(html).not.toContain("luna-progress");
  });

  /**
   * 徽标只跟随服务端判定的档位（协议 v12 每一维必填的 `alert`）——阈值（80% / 95%）只在服务端一份，
   * 前端不重算，否则页面、阈值通知与报错三处迟早对同一个额度各说一套（ADR-121 ⑥⑦）。
   */
  it("压力徽标只跟随服务端下发的档位，且只贴最紧的第一行", () => {
    const withAlert = renderUsage([
      {
        kind: "subscription",
        period: "monthly",
        dimensions: [
          { dimension: "tokens_in", limit: 1000, used: 970, alert: "p95" },
          { dimension: "tokens_out", limit: 1000, used: 900, alert: "p80" },
        ],
      },
    ]);
    expect(withAlert).toContain(translateForTest("cloud:usage.alert.p95"));
    expect(withAlert).toContain('data-alert="p95"');
    expect(withAlert, "第二紧的那一行不该也贴徽标（ADR-121 ⑥）").not.toContain('data-alert="p80"');

    // 同样的 97%，服务端说 none 就不该有徽标——阈值不在前端。
    const withoutAlert = renderUsage([
      {
        kind: "subscription",
        period: "monthly",
        dimensions: [{ dimension: "tokens_in", limit: 1000, used: 970, alert: "none" }],
      },
    ]);
    expect(withoutAlert).not.toContain("data-alert");
  });

  /** 三档徽标的中英文案齐备——缺一个就会在那一行画出 `usage.alert.p80`。 */
  it("三档压力徽标在中英资源包里都有文案", () => {
    for (const bundle of [cloudZh, cloudEn]) {
      const alert = (bundle.usage as Record<string, unknown>).alert as Record<string, unknown>;
      for (const level of ["p80", "p95", "exhausted"]) {
        expect(alert[level]).toBeTypeOf("string");
      }
    }
  });

  // ---- 门禁：服务端下发的标识符必须有中英文案（ADR-121 ③）----

  /**
   * **这是本轮那个缺陷的门禁，不是一条普通用例。**
   *
   * `t('<ns>.<服务端下发的值>', { defaultValue: 中性兜底 })` 这条链上，缺文案**不会报错、
   * 不会红**——只会把机器标识符画在用户眼前。四条权益 4/4 都在兜底就是这么来的：兜底
   * 裁决是对的，而 `entitlement` 这个命名空间从来不存在，且既有的 i18n key 存在性校验
   * 查不到它（那个 key 只在运行时拼出来）。
   *
   * 故这张清单必须**逐字取自服务端契约**，两种语言都查。新增一个服务端取值时，这里
   * 变红是唯一会提醒人补文案的东西。
   */
  it("服务端契约里的每个标识符在中英资源包里都有文案", () => {
    const contract: Record<string, string[]> = {
      // `EntitlementSet::free()` 的四条（点号在 i18n 侧换成下划线，见 entitlementI18nKey）
      entitlement: ["devices_max", "catalog_refresh", "cloud_inference", "sync_enabled"],
      // PRD-13 §1.2 的三档（2026-08-14 定案：免费 / Pro / Max）+ 中性兜底
      tier: ["free", "pro", "max", "other"],
    };
    const usageKeys = [
      // UsageLedgerDto.kind
      "ledger_subscription",
      "ledger_prepaid",
      "ledger_other",
      // UsageLedgerDto.period
      "period_none",
      "period_daily",
      "period_weekly",
      "period_monthly",
      // 维度的中性兜底（六个维度名本身由上面那条用例逐一查过）
      "dimension_other",
    ];
    for (const [locale, bundle] of [
      ["zh-CN", cloudZh],
      ["en-US", cloudEn],
    ] as const) {
      for (const [ns, keys] of Object.entries(contract)) {
        const group = (bundle as Record<string, unknown>)[ns] as Record<string, unknown>;
        expect(group, `${locale} 缺整个 ${ns} 命名空间——该命名空间下每个值都会露出裸标识符`)
          .toBeTypeOf("object");
        for (const key of keys) {
          expect(group[key], `${locale} 缺 ${ns}.${key}`).toBeTypeOf("string");
        }
      }
      const usage = (bundle as Record<string, unknown>).usage as Record<string, unknown>;
      for (const key of usageKeys) {
        expect(usage[key], `${locale} 缺 usage.${key}`).toBeTypeOf("string");
      }
    }
  });

  /** 退役的档位不留死文案（ADR-046）：`basic` / `metered` 对应不上任何服务端取值。 */
  it("不留对应不上服务端取值的档位文案", () => {
    for (const bundle of [cloudZh, cloudEn]) {
      const tier = (bundle as Record<string, unknown>).tier as Record<string, unknown>;
      expect(tier.basic).toBeUndefined();
      expect(tier.metered).toBeUndefined();
    }
  });

  // ---- 设备段（ADR-121 ②③）----

  function device(over: Partial<DeviceDto> = {}): DeviceDto {
    return {
      id: "d1",
      name: "MacBook-Pro",
      platform: "macos",
      status: "active",
      created_at: "2026-09-01T00:00:00Z",
      last_seen_at: "2026-09-10T00:00:00Z",
      current: true,
      ...over,
    };
  }

  function renderSignedIn(
    devices: DeviceDto[],
    entitlements: EntitlementsDto | null,
    loggingOut = false,
    sectionErrors: Record<string, unknown> = {},
    confirmingDelete = false,
  ): string {
    return renderWithSettingsHosts(
      createElement(SignedInView, {
        profile: { account_id: "acc", email: "a@b.c" },
        server: { origin: "https://ai.corp.example", display_name: "Corp", kind: "organization" },
        sectionErrors,
        devices,
        usage: [],
        entitlements,
        confirmingDelete,
        onLogout: () => {},
        onSwitchServer: () => {},
        loggingOut,
        onRevoke: () => {},
        onDeleteRequest: () => {},
        onDeleteCancel: () => {},
        onDeleteConfirm: () => {},
        onRefresh: () => {},
        formatTime: () => "—",
      } as never),
    );
  }

  it("身份段在「退出登录」旁给「切换服务器」", () => {
    expect(renderSignedIn([], null, false)).toContain(translateForTest("cloud:actions.switch_server"));
  });

  /** 严格本地挡住三块在线数据：只说一次并给隐私档入口；别的失败照说；未知值不写成破折号（F28）。 */
  it("严格本地：在线区块只提示一次，未知值写明暂不可获取", () => {
    const blocked = { code: "strict_local_blocked" };
    const html = renderSignedIn([], null, false, {
      devices: blocked,
      entitlements: blocked,
      usage: blocked,
    });
    const notice = translateForTest("cloud:session.strict_local_blocked");
    expect(html.split(notice).length - 1).toBe(1);
    expect(html).toContain(translateForTest("cloud:session.open_privacy"));
    expect(html).toContain(translateForTest("cloud:session.unknown"));
    expect(html).not.toContain(translateForTest("cloud:usage.empty"));
    expect(html).not.toContain(">—<");
    // 设备段同样写明暂不可获取，而不是空着。
    expect(html.split(translateForTest("cloud:session.unknown")).length - 1).toBeGreaterThanOrEqual(4);

    const mixed = renderSignedIn([], null, false, { devices: blocked, usage: { code: "network", host: "h", kind: "other", detail: "" } });
    expect(mixed.split(notice).length - 1).toBe(1);
  });

  /** 2026-10-11 真机：连不上服务端时三块各报一遍同一句。同码合并为一处；不同码各说各的。 */
  it("几块同一个失败只说一次，各块写暂不可获取", () => {
    const network = { code: "network", host: "10.0.0.62:8443", kind: "unreachable", detail: "" } as const;
    const html = renderSignedIn([], null, false, { devices: network, entitlements: network, usage: network });
    const line = errorMessage(network, translateCloud);
    expect(html.split(line).length - 1).toBe(1);
    expect(html).toContain(translateForTest("cloud:session.unknown"));
    expect(html).not.toContain(translateForTest("cloud:usage.empty"));
    expect(html).not.toContain(translateForTest("cloud:devices.empty"));

    const single = renderSignedIn([], null, false, { usage: network });
    expect(single.split(line).length - 1).toBe(1);
  });

  /**
   * 登出在途时按钮必须说出它在做事，并且不能再点第二次。
   *
   * 2026-09-27 真机：部署机数据库坏了，吊销请求卡住，「退出登录」点下去半分钟毫无反馈，
   * 用户只能认定按钮坏了。现在吊销有时限（`CloudClient::logout`），这几秒由按钮说出来。
   */
  /**
   * 开着同步时退出前先问一句，并说清：同步当场停、本地文件不动、再登录同一账户自动接着同步
   * （ADR-151 修订 2026-10-15）。没在同步就不多这一步。
   */
  it("开着同步时退出要确认，并写明再登录同一账户会自动接着同步", () => {
    expect(logoutConfirmation(false, translateCloud)).toBeNull();
    const asked = logoutConfirmation(true, translateCloud);
    expect(asked?.title).toBe(translateForTest("cloud:session.logout_confirm.title"));
    expect(asked?.description).toBe(translateForTest("cloud:session.logout_confirm.body"));
    expect(asked?.destructive).toBe(false);
    expect(cloudZh.session.logout_confirm.body).toContain("再次登录同一账户会自动接着同步");
    expect(cloudEn.session.logout_confirm.body).toMatch(/same account resumes syncing/);
  });

  it("切换服务器总要确认：写出退出的是哪台服务器上的哪个账户；之后「登录到」默认另一侧", () => {
    const test = { origin: "https://10.0.0.62:8443", display_name: "测试服务端", kind: "organization" } as const;
    const asked = switchServerConfirmation("jo@gmail.com", test, translateCloud);
    expect(asked.title).toBe(translateForTest("cloud:session.switch_confirm.title"));
    expect(asked.description).toContain("jo@gmail.com");
    expect(asked.description).toContain("测试服务端");
    expect(asked.destructive).toBe(false);
    expect(choiceAfterSwitchingFrom(test)).toBe("official");
    expect(choiceAfterSwitchingFrom({ origin: "https://api.lunarca.app", display_name: null, kind: "official" })).toBe("other");
    for (const bundle of [cloudZh, cloudEn]) {
      expect(bundle.session.switch_confirm.body).toContain("{{server}}");
      expect(bundle.actions.switch_server.length).toBeGreaterThan(0);
    }
  });

  it("登出在途时按钮显示「正在退出」并禁用", () => {
    const idle = renderSignedIn([device()], null);
    expect(idle).toContain(`>${translateForTest("cloud:actions.logout")}<`);
    expect(idle).not.toContain(translateForTest("cloud:actions.logging_out"));

    const busy = renderSignedIn([device()], null, true);
    expect(busy).toContain(translateForTest("cloud:actions.logging_out"));
    expect(busy).toMatch(/<button[^>]*disabled=""[^>]*aria-busy="true"/);
  });

  /**
   * `devices.max` 的落点是**设备段的标题**，不是权益清单里的一行（ADR-121 ②）。
   * 同行（Setapp / Dropbox）都把「几台里的第几台」放在设备管理的入口处。
   */
  it("设备上限出现在设备段标题上，不再占一行权益", () => {
    const html = renderSignedIn([device(), device({ id: "d2", current: false })], ents(["devices.max", 4]));
    expect(html).toContain(translateForTest("cloud:devices.title_with_limit", { used: 2, limit: 4 }));
    // 它不再作为权益行出现——同一件事一屏只说一次。
    expect(html).not.toContain(translateForTest("cloud:entitlement.devices_max"));
  });

  /** 失败软着陆：服务端没下发上限时标题退回「设备」，不写 `2 / NaN`。 */
  it("服务端没给设备上限时标题退回「设备」", () => {
    const html = renderSignedIn([device()], ents(["cloud.inference", false]));
    expect(html).toContain(`>${translateForTest("cloud:devices.title")}<`);
    expect(html).not.toContain("NaN");
  });

  /** 值不是整数时当没有上限——画一个解析不出的上限比不画更糟。 */
  it("设备上限的值不是整数时当没有上限", () => {
    const html = renderSignedIn([device()], ents(["devices.max", "many"]));
    expect(html).toContain(`>${translateForTest("cloud:devices.title")}<`);
  });

  /**
   * `platform` 是 Rust 的 `std::env::consts::OS`——登记过才显示，认不出就整段不写。
   * 此前它被原样拼进设备说明，于是屏幕上写着 `macos · 最近活跃：…`。
   */
  it("设备平台显示登记文案，认不出的平台整段不写", () => {
    const known = renderSignedIn([device({ platform: "macos" })], null);
    expect(known).toContain(translateForTest("cloud:devices.platform.macos"));
    expect(known, "内部标识漏进了界面").not.toContain("macos ·");

    const unknown = renderSignedIn([device({ platform: "freebsd" })], null);
    expect(unknown).not.toContain("freebsd");
  });

  /** 三个平台的中英文案齐备——缺一个那台设备就少半行说明。 */
  it("三个平台在中英资源包里都有文案", () => {
    for (const bundle of [cloudZh, cloudEn]) {
      const platform = ((bundle as Record<string, unknown>).devices as Record<string, unknown>)
        .platform as Record<string, unknown>;
      for (const os of ["macos", "windows", "linux"]) {
        expect(platform[os], `缺 devices.platform.${os}`).toBeTypeOf("string");
      }
    }
  });

  /**
   * 设备上限的值解析不出来时**不能把它吞掉**。过滤的判据是「真的被设备段标题消费了」，
   * 不是「叫这个名字」——否则一条服务端下发的权益会在界面上整个消失。
   */
  it("设备上限的值用不了时，它仍作为一条权益露出来", () => {
    const html = renderUsage([], undefined, ents(["devices.max", "many"]));
    expect(html).toContain(translateForTest("cloud:entitlement.devices_max"));
  });

  // ---- 批次 3：三段 + 危险区 + 两条真机发现 ----

  /** 段 1 只放身份；档位与租约属于「订阅与用量」那一段（ADR-121 ①）。 */
  it("账户段只放身份，档位与租约在订阅与用量段里", () => {
    const html = renderSignedIn([device()], ents(["cloud.inference", true]));
    const accountIdx = html.indexOf(translateForTest("cloud:session.email_label"));
    const tierIdx = html.indexOf(translateForTest("cloud:session.tier_label"));
    const usageIdx = html.indexOf(translateForTest("cloud:usage.title"));
    expect(accountIdx).toBeGreaterThan(-1);
    expect(usageIdx).toBeGreaterThan(accountIdx);
    expect(tierIdx, "档位仍留在身份段里").toBeGreaterThan(usageIdx);
  });

  /** 危险区不是第四节：它没有段标题那枚原语，改由分隔线与次要色降权。 */
  it("删除账户在页尾危险区，不作为第四个分节", () => {
    const html = renderSignedIn([device()], null);
    expect(html).toContain("cloud-account__danger-zone");
    // 三个段标题：账户 / 订阅与用量 / 设备——危险区不在其中。
    // 两个段标题：订阅与用量 / 设备。身份段刻意没有标题（页头已经写着「账户」），
    // 危险区也不在其中。
    expect(html.match(/luna-settings-panel__section-title/g) ?? []).toHaveLength(2);
    expect(html).toContain(translateForTest("cloud:delete.button"));
  });

  it("删除首层保留不可逆后果，确认时直接展示完整范围", () => {
    const normal = renderSignedIn([], null);
    expect(normal).toContain(translateForTest("cloud:delete.warning_short"));
    expect(normal).not.toContain(translateForTest("cloud:delete.warning"));
    const confirming = renderSignedIn([], null, false, {}, true);
    expect(confirming).toContain(translateForTest("cloud:delete.warning"));
    expect(confirming).toContain(translateForTest("cloud:delete.confirm"));
    expect(confirming).toContain(translateForTest("cloud:delete.cancel"));
  });

  /**
   * 真机 2026-09-10：离线租约那行写着「未验签（no_pubkey）」。
   * 六个码都是我们自己的（`lease_error_code`），登记过才说出来；认不出只说「未验签」。
   */
  it("租约未验签说出人能读的原因，不写内部码", () => {
    const withReason = renderSignedIn([device()], {
      tier: "free",
      entitlements: [],
      lease_verified: false,
      lease_expires_at: null,
      lease_error: "no_pubkey", capabilities: null, billing_standing: { state: "good" }, free_quota: { state: "not_applicable" },
    });
    expect(withReason).toContain(translateForTest("cloud:session.lease_reason.no_pubkey"));
    expect(withReason, "内部码漏进了界面").not.toContain("no_pubkey");

    const unknown = renderSignedIn([device()], {
      tier: "free",
      entitlements: [],
      lease_verified: false,
      lease_expires_at: null,
      lease_error: "something_new", capabilities: null, billing_standing: { state: "good" }, free_quota: { state: "not_applicable" },
    });
    expect(unknown).toContain(translateForTest("cloud:session.lease_unverified_plain"));
    expect(unknown).not.toContain("something_new");
  });

  /** 六个租约码的中英文案齐备——缺一个那一行就会露出内部码。清单取自 `lease_error_code`。 */
  it("六个租约错误码在中英资源包里都有文案", () => {
    const codes = ["no_pubkey", "malformed", "unsupported_alg", "bad_signature", "wrong_use", "expired"];
    for (const bundle of [cloudZh, cloudEn]) {
      const reason = ((bundle as Record<string, unknown>).session as Record<string, unknown>)
        .lease_reason as Record<string, unknown>;
      for (const code of codes) {
        expect(reason[code], `缺 session.lease_reason.${code}`).toBeTypeOf("string");
      }
    }
  });

  /**
   * 真机 2026-09-10：服务端给**订阅**账本下发的 `period` 也是 `none`，于是标题写成
   * 「套餐额度 · 不过期」——一句读不通的话。`none` 按账本种类分说。
   */
  it("period=none 按账本种类分说", () => {
    const sub = renderUsage([
      { kind: "subscription", period: "none", dimensions: [{ dimension: "tokens_in", limit: null, used: 0, alert: "none" }] },
    ]);
    expect(sub).toContain(translateForTest("cloud:usage.period_none_subscription"));
    expect(sub).not.toContain(translateForTest("cloud:usage.period_none_prepaid"));

    const pre = renderUsage([
      { kind: "prepaid", period: "none", dimensions: [{ dimension: "tokens_in", limit: null, used: 0, alert: "none" }] },
    ]);
    expect(pre).toContain(translateForTest("cloud:usage.period_none_prepaid"));
  });

  /**
   * 真机 5 / 2：五台活跃设备、上限两台，而屏幕上除了那两个数字**什么都没说**。
   * 「为什么会这样、我现在该做什么」恰恰是用户此刻唯一想知道的事。
   */
  it("设备超出上限时给出可执行的下一步，没超时不打扰", () => {
    const over = renderSignedIn(
      [device({ id: "a" }), device({ id: "b", current: false }), device({ id: "c", current: false })],
      ents(["devices.max", 2]),
    );
    expect(over).toContain(translateForTest("cloud:devices.over_limit"));

    const within = renderSignedIn([device()], ents(["devices.max", 2]));
    expect(within).not.toContain(translateForTest("cloud:devices.over_limit"));
  });

  /**
   * 设备名由各机器自报，同型号的几台一字不差（真机：五条都叫
   * `Weijias-MacBook-Pro-M5.local`）。加入时间是唯一能把它们分开、也是决定「撤销哪一台」
   * 要看的那个事实，而 `created_at` 此前一个消费者都没有。
   */
  it("设备行写出加入时间，同名设备才分得开", () => {
    const html = renderSignedIn([device({ created_at: "2026-09-01T00:00:00Z" })], null);
    expect(html).toContain(
      translateForTest("cloud:devices.joined", {
        time: formatInstant("2026-09-01T00:00:00Z", "zh-CN"),
      }),
    );
  });

  /** 页头已经写着「账户」，身份段再贴一遍就是同屏重复（PRD-09-070 ①）。 */
  it("身份段没有段标题", () => {
    const html = renderSignedIn([device()], null);
    expect(html).toContain(translateForTest("cloud:session.email_label"));
    expect(html, "身份段贴了一个与页头重复的标题").not.toContain(
      `>${translateForTest("cloud:session.title")}<`,
    );
  });

  /**
   * 真机 2026-09-10：登出再登入后显示「设备（2 / 2）」，而其中一条是**已撤销**。
   * 分子只数还能用凭据的那些——把撤销的算进去等于告诉用户「你占满了」。
   */
  it("设备计数不把已撤销的算进分子", () => {
    const html = renderSignedIn(
      [
        device({ id: "old", current: false, status: "revoked" }),
        device({ id: "new", current: true, status: "active" }),
      ],
      ents(["devices.max", 2]),
    );
    expect(html).toContain(translateForTest("cloud:devices.title_with_limit", { used: 1, limit: 2 }));
    expect(html).not.toContain(translateForTest("cloud:devices.title_with_limit", { used: 2, limit: 2 }));
    // 已撤销的仍然列出来——不数它，但也不藏它。
    expect(html).toContain(translateForTest("cloud:devices.revoked"));
  });
});
