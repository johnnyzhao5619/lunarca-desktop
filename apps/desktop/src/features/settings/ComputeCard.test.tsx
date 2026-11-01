/**
 * 「AI 在哪跑」卡片守护。
 *
 * **拦的是三类静默失败**：
 *
 * 1. 严格本地下渲染出「录音转写也交给服务端」勾选框。它此刻恒为假（Rust 侧 `location`
 *    为 `strict_local` 时 `transcription_on_server` 必为 false），于是界面呈现的是一个
 *    勾不动、也解释不了为什么勾不动的开关——用户读到的是"软件坏了"。
 * 2. 落点新增一种而文案没跟上：`LOCATION_COPY` 的穷尽性由 tsc 保证，**键在不在资源包里**
 *    只有这里能验。缺 key 时 i18next 回落成裸 key，界面上是一行 `compute_card.xxx`。
 * 3. 没有本机兜底模型时不给警示。服务端断线那一刻转写会直接停摆，而这是唯一的预告位。
 */

import { createElement } from "react";
import { describe, expect, it } from "vitest";

import en from "../../i18n/locales/en-US/settings.json";
import zh from "../../i18n/locales/zh-CN/settings.json";
import {
  ComputeCardView,
  LOCAL_ONLY_COPY,
  SETUP_INCOMPLETE_COPY,
  LOCATION_COPY,
  runsOnServer,
} from "./ComputeCard";
import { TRANSCRIPTION_CONSENT_KEY } from "./TranscriptionConsentRow";
import {
  createSettingSpecFixture,
  renderWithSettingsHosts,
  translateForTest,
} from "./settings-test-fixture";
import type {
  ComputeLocation,
  ComputeRouteDto,
  ComputeStatusDto,
  LocalCandidateDto,
  SettingValueDto,
} from "../../ipc/bindings";

/** 造一条路由行；三条能力各自的执行位置与理由现在由 Rust 逐条下发（ADR-059 ④）。 */
function route(
  capability: string,
  location: ComputeRouteDto["location"],
  reason: ComputeRouteDto["reason"],
  canUseServer = true,
): ComputeRouteDto {
  const slot =
    capability === "chat"
      ? "default_llm"
      : capability === "embedding"
        ? "default_embedding"
        : "realtime_asr";
  return {
    capability,
    location,
    reason,
    setting_key: `ai.assignment.${slot}`,
    can_use_server: canUseServer,
  };
}

function localCandidate(
  capability: LocalCandidateDto["capability"],
  overrides: Partial<LocalCandidateDto> = {},
): LocalCandidateDto {
  const slot =
    capability === "chat"
      ? "default_llm"
      : capability === "embedding"
        ? "default_embedding"
        : "realtime_asr";
  return {
    capability,
    policy: capability === "embedding" ? "explicit_only" : "unreachable_before_visible_output",
    model_name: capability === "transcription" ? "Whisper Small" : "Qwen3 0.6B",
    readiness: "ready",
    is_minimal: false,
    setting_key: `ai.assignment.${slot}`,
    ...overrides,
  };
}

function status(overrides: Partial<ComputeStatusDto> = {}): ComputeStatusDto {
  return {
    location: "on_device",
    routes: [
      route("chat", "device", "device_policy"),
      route("embedding", "device", "device_policy"),
      route("transcription", "device", "device_policy"),
    ],
    local_candidates: [
      localCandidate("chat"),
      localCandidate("embedding", { model_name: null, readiness: "not_applicable" }),
      localCandidate("transcription"),
    ],
    pending_handover: [],
    chat_route_kind: "on_device",
    transcription_server_blocker: null,
    outbound_blocked_by_strategy: false,
    server_setup_incomplete: false,
    ...overrides,
  };
}

/**
 * 转写同意那一键的注册表声明。
 *
 * 走仓库既有的 spec 夹具工厂，只覆盖这一项与别处不同的几个字段——手搓一份带
 * `as unknown` 的等于第二个事实源。控件形态刻意用注册表说的 `toggle`（ADR-122 ⑤）：
 * 夹具里写死别的 kind，就是在测一件注册表没说过的事。
 */
const CONSENT_SPEC = createSettingSpecFixture({
  key: TRANSCRIPTION_CONSENT_KEY,
  group: "provider_model",
  // 该页零分节（ADR-122 ⑦）：注册表里这些键的 `section` 都是 None。
  section: null,
  surface: { kind: "panel", route: "settings://ai_services/provider.remote_asr_enabled" },
  current_value: { kind: "bool", value: true },
  i18n_key: "settings:provider.remote_asr_enabled",
  desc_i18n_key: "settings:provider.remote_asr_enabled_desc",
});

/**
 * 服务端形态的完整夹具：落点在服务端时三条路由**也**在服务端。
 *
 * 只改 `location` 而留着 `device` 路由的夹具描述的是「全部显式钉在本机」，
 * 而本机备用只对「这条能力在服务端跑」才有意义——两者混用会让用例测到一件
 * 线上不成立的组合。
 */
function serverStatus(overrides: Partial<ComputeStatusDto> = {}): ComputeStatusDto {
  return status({
    location: "official_cloud",
    routes: [
      route("chat", "server", "server_default"),
      route("embedding", "server", "server_default"),
      route("transcription", "server", "explicit_server"),
    ],
    ...overrides,
  });
}

/**
 * 优先落点那一行的注册表声明。
 *
 * 它与同意项**必须是两份**：`specOf` 此前对任何 key 都回同一个 spec，于是卡上那两行
 * 渲染出同一个标签与同一句说明——「这两行说的是不是同一件事」这类断言在那种夹具下
 * 一概失效（写本轮的负向断言时当场撞上）。
 */
const STRATEGY_SPEC = createSettingSpecFixture({
  key: "ai.route_strategy",
  group: "provider_model",
  section: null,
  surface: { kind: "panel", route: "settings://ai_services/ai.route_strategy" },
  current_value: { kind: "str", value: "cloud_first" },
  default_value: { kind: "str", value: "cloud_first" },
  i18n_key: "settings:ai.route_strategy",
  desc_i18n_key: "settings:ai.route_strategy_desc",
  control: {
    kind: "select",
    options: ["cloud_first", "local_first", "local_first_fallback", "local_only"],
    style: "dropdown",
  },
});

const SPECS = new Map([
  [TRANSCRIPTION_CONSENT_KEY, CONSENT_SPEC],
  ["ai.route_strategy", STRATEGY_SPEC],
]);

function render(dto: ComputeStatusDto, serverName: string | null = null): string {
  return renderWithSettingsHosts(
    createElement(ComputeCardView, {
      status: dto,
      setValue: async (_key, value) => value,
      specOf: (key) => SPECS.get(key),
      resolveHandover: async () => {},
      serverName,
    }),
  );
}

/**
 * 完整设置投影：逐项那张表常驻。
 *
 * 标准投影下它**不在 DOM 里**（折叠是受控渲染，不是 `details` 的隐藏内容），所以断言
 * 表内容的用例必须在这一档渲染——否则断言的是"它没渲染"，永远绿不了也永远红不了。
 */
function renderAdvanced(dto: ComputeStatusDto): string {
  return renderWithSettingsHosts(
    createElement(ComputeCardView, {
      status: dto,
      setValue: async (_key: string, value: SettingValueDto) => value,
      specOf: (key: string) => SPECS.get(key),
      resolveHandover: async () => {},
      showingAdvanced: true,
    }),
  );
}

/**
 * 「这段文案真的在屏幕上」——**并且它真的是一段文案**。
 *
 * 裸 `toContain(translateForTest(key))` 有一个静默失效档：key 被删掉时 i18next 回落成
 * key 本身，而界面渲染的也是同一个回落串，于是断言照样绿、用户看到的是一行裸 key。
 * 本轮就撞上了——`compute_card.transcription_consent` 删掉之后，那条断言靠
 * `…_consent_hint` 这个回落串的前缀继续通过。故先证明它译得出来，再证明它在屏幕上。
 */
function expectRealCopy(html: string, key: string): void {
  const text = translateForTest(key);
  expect(text, `${key} 在资源包里不存在——断言比对的是回落的裸 key`).not.toContain(key.split(":")[1]);
  expect(html, `屏幕上没有 ${key}`).toContain(text);
}

/** 资源包里按点号路径取值；缺任何一段即视为缺 key。 */
function lookup(bundle: unknown, path: string): unknown {
  return path.split(".").reduce<unknown>((node, part) => {
    if (node === null || typeof node !== "object") return undefined;
    return (node as Record<string, unknown>)[part];
  }, bundle);
}

describe("算力落点卡片", () => {
  it("逐能力显示真实执行位置，而不是把已连接服务端写成全量接管", () => {
    const html = renderAdvanced(
      status({
        location: "organization",
        routes: [
          route("chat", "device", "explicit_device"),
          route("embedding", "server", "server_default"),
          route("transcription", "device", "needs_consent"),
        ],
      }),
    );
    expect(html).toContain(translateForTest("settings:compute_card.route.chat"));
    expect(html).toContain(translateForTest("settings:compute_card.route.embedding"));
    expect(html.match(/data-location="local"/g)).toHaveLength(2);
    expect(html.match(/data-location="server"/g)).toHaveLength(1);
  });

  /**
   * **与卡头同屏，就必须是同一个词**（第一轮评审第 9 条）。
   *
   * 卡头写「AI 请求发往 Lunarca 云」而格里写「服务端托管」时，用户读不出这两句说的是
   * 同一个地方。
   */
  it("在服务端跑的那几格，名字跟着卡头的落点走", () => {
    const cloud = renderAdvanced(serverStatus({ location: "official_cloud" }));
    expectRealCopy(cloud, "settings:compute_card.route.server_cloud");
    expect(cloud, "「服务端托管」这个泛称已从算力卡退役").not.toContain("服务端托管");

    const org = renderAdvanced(serverStatus({ location: "organization" }));
    expectRealCopy(org, "settings:compute_card.route.server_organization");
  });

  /**
   * **标准投影下逐项表收起，那句总结必须替它把隐私事实说完**（ADR-127）。
   *
   * Apple Intelligence 的设置里只有一个开关、执行位置要去另一个默认关闭的报告里看——
   * 那一点正是它被批评的地方（用户无法在当下判断这次请求出没出设备）。这条断言钉住
   * 「三条能力一条都不能漏出那句话」：漏掉的那条恰恰是用户不知道自己不知道的。
   */
  it("标准投影下收起逐项表，但那句总结说全了三条能力去哪", () => {
    const html = render(
      status({
        location: "official_cloud",
        routes: [
          route("chat", "server", "server_default"),
          route("embedding", "server", "server_default"),
          route("transcription", "device", "needs_consent"),
        ],
      }),
    );
    // 折叠是受控渲染：收起时逐项表根本不在 DOM 里（`details` 那一版是渲染了再藏），
    // 于是断言的是"入口在、表不在"。
    expect(html, "标准投影下该只留卡头那颗入口").toContain("luna-compute-card__routes-toggle");
    expect(html, "标准投影下逐项表不该渲染").not.toContain('class="luna-compute-card__routes"');
    for (const capability of ["chat", "embedding", "transcription"]) {
      expectRealCopy(html, `settings:compute_card.route.inline.${capability}`);
    }
    // 两侧都有时用的是混合句，而不是把某一侧悄悄省掉。
    expect(html).toContain(translateForTest("settings:compute_card.route.server_cloud"));

    // 完整设置下逐项表常驻，那颗入口随之消失——同一件事不给两个开关。
    const advanced = renderAdvanced(status({ location: "official_cloud" }));
    expect(advanced).not.toContain("luna-compute-card__routes-toggle");
    expect(advanced).toContain('class="luna-compute-card__routes"');
  });

  /**
   * 全本机形态那句话必须说出「不发往服务端」——它是这一档唯一的卖点。
   *
   * 期望值在这里**整句拼出来**（含 `Intl.ListFormat` 的连接结果）而不是比对半截：
   * 带占位符的 key 直接拿去 `toContain` 会比对未插值的模板串，那种断言永远不会红。
   */
  it("全本机形态的总结句说明不外发", () => {
    const names = ["chat", "embedding", "transcription"].map((capability) =>
      translateForTest(`settings:compute_card.route.inline.${capability}`),
    );
    // 与实现同一档：默认档（long conjunction）。躲连接词这件事由能力名自带顿号解决，
    // 不靠 `narrow`——WKWebView 与 Node 的 ICU 数据在那一档上并不一致（见 `summaryLines` 头注）。
    const names2 = new Intl.ListFormat("zh-CN", { style: "long", type: "conjunction" }).format(names);
    expect(render(status({ location: "on_device" }))).toContain(
      translateForTest("settings:compute_card.summary.local_only", { names: names2 }),
    );
  });

  it("每种落点的标题与说明在中英资源包里都存在", () => {
    for (const [location, copy] of [
      ...Object.entries(LOCATION_COPY),
      ["local_only", LOCAL_ONLY_COPY] as const,
      ["setup_incomplete", SETUP_INCOMPLETE_COPY] as const,
    ]) {
      for (const key of [copy.name, copy.body]) {
        expect(lookup(zh, key), `zh-CN 缺 ${key}（落点 ${location}）`).toBeTypeOf("string");
        expect(lookup(en, key), `en-US 缺 ${key}（落点 ${location}）`).toBeTypeOf("string");
      }
    }
  });

  /**
   * **两种服务落点的说明不得雷同**（ADR-125 ①）。
   *
   * 它们此前中英四份逐字相同，而 PRD-13 §1.4
   * 要官方云讲「零留存 / 不训练」、§1.7 ④
   * 要私有化讲「政策归部署方」且**绝不套用**官方云的承诺。复制粘贴回来时这条先红。
   */
  it("官方云与组织服务端各讲各的隐私承诺，不共用一句话", () => {
    for (const bundle of [zh, en]) {
      expect(lookup(bundle, "compute_card.official_cloud_body")).not.toBe(
        lookup(bundle, "compute_card.organization_body"),
      );
    }
  });

  /**
   * **指路只能指向此刻真的到得了的地方**（ADR-125 ⑭⑯）。
   *
   * 官方云那一段此前写着「能不能连上见下方『连接服务端』」，而那一块自己写的是
   * 「用 Lunarca 云的话可以忽略它」——同屏两句话互相打架。指路属于组织形态。
   * 本机备用的 `explicit_only` 同理指过「模型分配」，那一页签只在完整设置下存在。
   */
  it("指路不指向当前形态下够不着的地方", () => {
    for (const [bundle, connection, assignments] of [
      [zh, "连接服务端", "模型分配"],
      [en, "Server connection", "Model assignments"],
    ] as const) {
      expect(
        lookup(bundle, "compute_card.official_cloud_body"),
        "官方云指向了它自己说可以忽略的那一块",
      ).not.toContain(connection);
      expect(
        lookup(bundle, "compute_card.organization_body"),
        "连接卡已承担入口，运行卡不重复指路",
      ).not.toContain(connection);
      expect(
        lookup(bundle, "compute_card.local_candidate.policy.explicit_only"),
        "标准投影下没有「模型分配」这一页签",
      ).not.toContain(assignments);
    }
  });

  it("严格本地下不渲染转写同意勾选框——那是个勾不动的开关", () => {
    const html = render(status({ location: "strict_local" }));
    expect(html).not.toContain('type="checkbox"');
    expect(html).toContain(translateForTest("settings:compute_card.strict_local"));
  });

  /**
   * 纯本机形态下卡上**没有**「接入组织服务端」这颗按钮：紧挨着的「连接服务端」卡本身
   * 就是那个入口，此前同屏上下相邻两颗按钮做同一件事。
   */
  it("纯本机形态下不渲染同意勾选框——没有服务器可交", () => {
    const html = render(status({ location: "on_device" }));
    expect(html).not.toContain('type="checkbox"');
  });

  /**
   * 严格本地下「优先在哪运行」四档全部无意义：行照常呈现但不可用，原因写在说明里——
   * 与转写同意勾不动时同一形制，而不是一个改了没反应的下拉。
   */
  it("严格本地下优先落点那一行不可用，并说明原因", () => {
    const html = render(status({ location: "strict_local" }));
    expect(html).toContain('data-setting-key="ai.route_strategy"');
    expect(html).toContain('data-disabled="true"');
    expectRealCopy(html, "settings:compute_card.strategy_strict_local");
  });

  /**
   * 有下一步动作的状态都得给按钮。
   *
   * 严格本地下此前只有一句「先到『存储与隐私』里关闭它」而**没有入口**——用户得自己
   * 认出那是个分组名再去侧栏找。同一个仓库里 `SemanticIndexPanel` 早就在用完整的
   * 设置深链（切组 + 揭示高级 + 滚动 + 聚焦），卡片没有理由只做其中一段。
   */
  it("严格本地给出回到隐私档的入口，而不是让用户自己去找那个分组", () => {
    const html = render(status({ location: "strict_local" }));
    expect(html).toContain(translateForTest("settings:compute_card.action.strict_local"));
  });

  it("已经在服务端上且账户可用时不给多余的动作按钮——此刻没有「下一步」可言", () => {
    for (const location of ["official_cloud", "organization"] as const) {
      const html = render(status({ location, transcription_server_blocker: null }));
      expect(html, location).not.toContain("luna-compute-card__actions");
    }
  });

  it("服务端形态下渲染同意勾选框，且反映当前值", () => {
    const html = render(
      status({
        location: "official_cloud",
        routes: [
          route("chat", "server", "server_default"),
          route("embedding", "server", "server_default"),
          route("transcription", "server", "explicit_server"),
        ],
      }),
    );
    // 与全应用设置行同一枚开关**和同一枚行外壳**（ADR-122 ⑤ / ADR-125 ⑱）：
    // 注册表声明 `toggle`，标签与说明也取注册表那一份——卡上另造一套名字与说明，
    // 会与搜索结果里显示的那一份分叉。
    expect(html).toContain('role="switch"');
    expect(html).toContain('aria-checked="true"');
    expect(html, "面板自造了原生控件").not.toContain('type="checkbox"');
    expect(html, "同意项没走设置行外壳").toContain("luna-settings-field--group");
    expectRealCopy(html, "settings:provider.remote_asr_enabled");
    expectRealCopy(html, "settings:provider.remote_asr_enabled_desc");
  });

  it("本机模型缺失、能力未知与运行时不可达分别说清，而不伪装成模型未加载", () => {
    const html = renderAdvanced(
      serverStatus({
        local_candidates: [
          localCandidate("chat", { readiness: "runtime_unavailable", model_name: null }),
          localCandidate("embedding", { model_name: null, readiness: "not_applicable" }),
          localCandidate("transcription", { readiness: "model_missing", model_name: null }),
        ],
      }),
    );
    expect(html).toContain('data-readiness="runtime_unavailable"');
    expect(html).toContain('data-readiness="model_missing"');
    expect(html).toContain(translateForTest("settings:compute_card.local_candidate.readiness.runtime_unavailable"));
    expect(html).toContain(translateForTest("settings:compute_card.local_candidate.readiness.model_missing"));
  });

  it("落在本机却缺模型的能力在折叠表之外常显，并给出本地模型去处（F15/F16）", () => {
    const html = render(
      status({
        local_candidates: [
          localCandidate("chat", { readiness: "model_missing", model_name: null }),
          localCandidate("embedding", { model_name: null, readiness: "not_applicable" }),
          localCandidate("transcription"),
        ],
      }),
    );
    const names = translateForTest("settings:compute_card.route.inline.chat");
    expect(html).toContain(
      translateForTest("settings:compute_card.summary.unready_model_missing", { names }),
    );
    expect(html).toContain(translateForTest("settings:compute_card.action.local_models"));
  });

  it("在服务端跑的能力不因本机备用缺模型而报「还不能用」", () => {
    const html = render(
      serverStatus({
        local_candidates: [
          localCandidate("chat", { readiness: "model_missing", model_name: null }),
          localCandidate("embedding", { model_name: null, readiness: "not_applicable" }),
          localCandidate("transcription"),
        ],
      }),
    );
    expect(html).not.toContain("luna-compute-card__unready");
  });

  it("显示实际本机候选、换腿边界与嵌入的显式配置边界", () => {
    const html = renderAdvanced(
      serverStatus({
        location: "organization",
        local_candidates: [
          localCandidate("chat", { model_name: "qwen3.5:0.8b-mlx" }),
          localCandidate("embedding", { model_name: "bge-m3", readiness: "ready" }),
          localCandidate("transcription", { model_name: "Whisper Tiny" }),
        ],
      }),
    );
    expect(html).toContain("qwen3.5:0.8b-mlx");
    expect(html).toContain("bge-m3");
    expect(html).toContain(
      translateForTest("settings:compute_card.local_candidate.readiness.ready", { model: "bge-m3" }),
    );
    expect(html).toContain("Whisper Tiny");
    expect(html).toContain(
      translateForTest(
        "settings:compute_card.local_candidate.policy.unreachable_before_visible_output",
      ),
    );
    expect(html).toContain(
      translateForTest("settings:compute_card.local_candidate.policy.explicit_only"),
    );
  });

  it("没有显式本机嵌入分配时如实为不适用，而不是把服务端 profile 说成本机模型", () => {
    const html = renderAdvanced(
      serverStatus({
        local_candidates: [
          localCandidate("chat"),
          localCandidate("embedding", { model_name: null, readiness: "not_applicable" }),
          localCandidate("transcription"),
        ],
      }),
    );
    // `not_applicable` 那句「不参与自动切换」与 `explicit_only` 的策略句说的是同一件事，
    // 两句并排就是同一信息说两遍——故只渲染策略句（ADR-122 ③ 同一条：一屏只说一次）。
    expect(html).toContain(
      translateForTest("settings:compute_card.local_candidate.policy.explicit_only"),
    );
    expect(html).not.toContain(
      translateForTest("settings:compute_card.local_candidate.readiness.not_applicable"),
    );
  });

  /**
   * **一组能力只列一次**（ADR-122 ③）。
   *
   * `routes` 与 `local_candidates` 下发的是同样三条能力，此前前端分两张表各渲一遍——
   * 三个概念占了六行、六个跳转按钮，其中三个逐字相同（「查看本机模型」，真机截图可见）。
   */
  it("三条能力只列一次，每条一个入口", () => {
    const html = renderAdvanced(serverStatus());
    // 三条能力各一行，各一颗「更改」——不是六行六个按钮。
    expect(html.match(/luna-compute-card__route-switch/g)).toHaveLength(3);
    for (const capability of ["chat", "embedding", "transcription"]) {
      const label = translateForTest(`settings:compute_card.route.${capability}`);
      expect(html.split(`>${label}<`)).toHaveLength(2);
    }
    // 本机备用是行内的一个字段，不是第二张表。
    expect(html).toContain('data-role="fallback"');
    expect(html).toContain(translateForTest("settings:compute_card.local_candidate.title"));
    expect(html, "「查看本机模型」这个重复入口已退役").not.toContain(
      "luna-compute-card__candidate-action",
    );
  });

  /**
   * 全本机形态没有「本机备用」可言——备用就是它自己。但**用的是哪个模型、就绪没有**
   * 必须在屏幕上：此前在本机跑的能力什么模型名都不显示，而那恰是用户最想知道的；
   * 引擎没起、模型没装时这一页也一个字都不报。
   */
  it("全本机形态每条能力写出正在用的本机模型，而不是留白", () => {
    const html = renderAdvanced(status({ location: "on_device" }));
    expect(html).not.toContain('data-role="fallback"');
    expect(html.match(/data-role="active"/g)).toHaveLength(3);
    expect(html).toContain("Qwen3 0.6B");
    expect(html).toContain("Whisper Small");
    expectRealCopy(html, "settings:compute_card.local_candidate.active_title");
    expect(html.match(/luna-compute-card__route-switch/g)).toHaveLength(3);

    const broken = renderAdvanced(
      status({
        location: "on_device",
        local_candidates: [
          localCandidate("chat", { readiness: "runtime_unavailable", model_name: null }),
          localCandidate("embedding", { model_name: null, readiness: "not_applicable" }),
          localCandidate("transcription", { readiness: "model_missing", model_name: null }),
        ],
      }),
    );
    expect(broken).toContain('data-readiness="runtime_unavailable"');
    expect(broken).toContain(
      translateForTest("settings:compute_card.local_candidate.readiness.model_missing"),
    );
  });

  /**
   * 「优先在哪运行」的三档理由都要有文案：它们是 Rust 新下发的枚举值，缺 key 时界面上
   * 是一行裸 `compute_card.reason.strategy_local`。
   */
  it("路由策略带来的三种理由在中英资源包里都存在，并渲染成人话", () => {
    for (const reason of ["strategy_local", "strategy_local_only", "strategy_local_unavailable"]) {
      expect(lookup(zh, `compute_card.reason.${reason}`), `zh-CN 缺 ${reason}`).toBeTypeOf("string");
      expect(lookup(en, `compute_card.reason.${reason}`), `en-US 缺 ${reason}`).toBeTypeOf("string");
    }
    const html = renderAdvanced(
      status({
        location: "official_cloud",
        routes: [
          route("chat", "device", "strategy_local"),
          route("embedding", "server", "server_default"),
          route("transcription", "server", "server_default"),
        ],
      }),
    );
    expectRealCopy(html, "settings:compute_card.reason.strategy_local");
  });

  /**
   * 仅本机 + 连着服务端：卡头不能再说「请求发往云」，同意开关要说清为什么勾不动。
   */
  it("仅本机策略下卡头改口，转写同意按这个原因禁用", () => {
    const html = render(
      status({
        location: "official_cloud",
        outbound_blocked_by_strategy: true,
        transcription_server_blocker: "local_only_strategy",
        routes: [
          route("chat", "device", "strategy_local_only", false),
          route("embedding", "device", "strategy_local_only", false),
          route("transcription", "device", "strategy_local_only", false),
        ],
      }),
    );
    expectRealCopy(html, "settings:compute_card.local_only");
    expect(html).not.toContain(translateForTest("settings:compute_card.official_cloud"));
    expect(html).toContain('data-disabled="true"');
    expectRealCopy(html, "settings:compute_card.transcription_local_only");
  });

  /**
   * 服务端一个上游都没配（ADR-041 ①）：卡头说「它尚未配置算力」，不说「请求发往云」；逐项理由
   * 记在部署方头上，转写同意按这个原因禁用。它压过「仅本机」——请求本来就到不了那台服务端。
   */
  it("服务端尚未配置算力时卡头如实说，逐项与转写同意跟着改口", () => {
    const html = renderAdvanced(
      status({
        location: "official_cloud",
        server_setup_incomplete: true,
        outbound_blocked_by_strategy: true,
        transcription_server_blocker: "local_only_strategy",
        routes: [
          route("chat", "device", "server_not_configured", false),
          route("embedding", "device", "server_not_configured", false),
          route("transcription", "device", "server_not_configured", false),
        ],
      }),
    );
    expectRealCopy(html, "settings:compute_card.setup_incomplete");
    expect(html).not.toContain(translateForTest("settings:compute_card.official_cloud"));
    expect(html).not.toContain(translateForTest("settings:compute_card.local_only"));
    expectRealCopy(html, "settings:compute_card.reason.server_not_configured");

    const blocked = render(
      status({
        location: "official_cloud",
        server_setup_incomplete: true,
        transcription_server_blocker: "server_not_configured",
        routes: [route("transcription", "device", "server_not_configured", false)],
      }),
    );
    expect(blocked).toContain('data-disabled="true"');
    expectRealCopy(blocked, "settings:compute_card.reason.server_not_configured");
  });

  /**
   * 判据是 `location`，**不是「有没有服务器名」**：官方云背后是内置默认值，真机上它是一个
   * `10.0.0.x`——贴出来就成了「AI 请求发往 Lunarca 云」后面紧跟一台内网机器。
   */
  it("组织形态写出是哪一台，官方云不写——云没有「地址」这个概念", () => {
    const org = render(status({ location: "organization" }), "Corp · https://ai.corp.example");
    expect(org).toContain("https://ai.corp.example");
    const cloud = render(status({ location: "official_cloud" }), "https://10.0.0.62:8443");
    expect(cloud, "官方云贴出了内置默认地址").not.toContain("https://10.0.0.62:8443");
  });

  /**
   * 本机是主力却只有最小档时给出引导——避免用户拿 tiny 的质量判断整个产品。
   *
   * **两种落点下给的是两条不同的出路**（ADR-127 第 5 条）：连着服务端时，「开启服务端
   * 转写」就在同一张卡上，比下载一个 1.5 GB 的模型近得多；纯本机时那条路不存在，
   * 只说模型库。此前两档共用一句，把唯一那条转化路径浪费掉了。
   */
  it("本机只有最小档时给出引导，且连着服务端时先给同卡那条路", () => {
    const minimal = [
      localCandidate("chat"),
      localCandidate("embedding", { model_name: null, readiness: "not_applicable" }),
      localCandidate("transcription", { is_minimal: true }),
    ];
    for (const location of ["strict_local", "on_device"] as const) {
      expect(render(status({ location, local_candidates: minimal })), location).toContain(
        translateForTest("settings:compute_card.minimal_model_hint_local"),
      );
    }
    // 服务端形态 + 转写还在本机：那句要先指向同一张卡上的开关。
    const onServer = render(
      status({
        location: "official_cloud",
        routes: [
          route("chat", "server", "server_default"),
          route("embedding", "server", "server_default"),
          route("transcription", "device", "needs_consent"),
        ],
        local_candidates: minimal,
      }),
    );
    expectRealCopy(onServer, "settings:compute_card.minimal_model_hint");
  });

  /**
   * 判据是**转写这条能力**的落点，不是这台机器的落点：官方云用户没开转写同意时，
   * 主力就是本机 tiny，那句引导正该出现；转写真在服务端跑时才不提。
   */
  it("最小档引导跟着转写的落点走，而不是跟着机器的落点走", () => {
    const minimal = [
      localCandidate("chat"),
      localCandidate("embedding", { model_name: null, readiness: "not_applicable" }),
      localCandidate("transcription", { is_minimal: true }),
    ];
    const onServer = render(serverStatus({ local_candidates: minimal }));
    expect(onServer).not.toContain(translateForTest("settings:compute_card.minimal_model_hint"));

    const consentOff = render(
      status({
        location: "official_cloud",
        routes: [
          route("chat", "server", "server_default"),
          route("embedding", "server", "server_default"),
          route("transcription", "device", "needs_consent"),
        ],
        local_candidates: minimal,
      }),
    );
    expect(consentOff).toContain(translateForTest("settings:compute_card.minimal_model_hint"));
  });

  /** 未登录 = 全在本机：给一个去账户页的入口（登录是唯一入口，ADR-155）。 */
  it("未登录时只给去账户页登录这一颗", () => {
    const html = render(status({ location: "on_device" }));
    expectRealCopy(html, "settings:compute_card.action.sign_in");
  });

  it("账户可用时同意项正常可勾，且不再提登录", () => {
    const html = render(
      status({ location: "official_cloud", transcription_server_blocker: null }),
    );
    expect(html).toContain('data-disabled="false"');
    expect(html).not.toContain(translateForTest("settings:compute_card.action.sign_in"));
  });

  /**
   * 每条能力必须同时说清**在哪跑**与**为什么是它**（ADR-059 ④）。
   *
   * 真机反馈（2026-08-11）：卡片只写「这台设备 / 服务端」。同样是「这台设备」，可能是用户
   * 自己钉的、隐私档压的、还差一次同意、或服务端根本不提供——四种情况的下一步动作完全
   * 不同，而卡片给的是同一个词，于是用户只能逐个槽位翻设置才能发现原因。
   */
  it("每条能力都写出理由，并给一个就地切换入口（ADR-059）", () => {
    const html = renderAdvanced(
      status({
        location: "organization",
        routes: [
          route("chat", "device", "explicit_device"),
          route("embedding", "server", "server_default"),
          route("transcription", "device", "needs_consent"),
        ],
      }),
    );
    for (const reason of ["explicit_device", "server_default", "needs_consent"]) {
      expect(html, `缺「为什么是它」：${reason}`).toContain(
        translateForTest(`settings:compute_card.reason.${reason}`),
      );
    }
    // 就地切换：深链直接落到决定这条能力的那个槽位，而不是把用户丢到设置页顶端。
    expect(html).toContain(translateForTest("settings:compute_card.switch"));
  });

  /**
   * **三格的「更改」只能有一种去处**（ADR-126 第 5 条）。
   *
   * 此前对话与语义索引落到模型库 sheet，而转写落到本卡下方那枚同意开关——同一个词、
   * 同一个位置、两种完全不同的结果（真机 2026-09-12 用户圈点）。改法一度是「服务端
   * 形态下转写那一格不给按钮」，真机当场再次否掉：三格里两格有入口一格没有，不统一
   * 原样还在。现在三格一律给，且都只回答「用哪个模型」——「要不要交给服务端」是另一
   * 件事，由下方那枚同意开关负责。
   */
  it("三格的「更改」一律落到模型分配那一行，没有一颗指向同意开关", () => {
    for (const dto of [
      status({
        location: "official_cloud",
        routes: [
          route("chat", "server", "server_default"),
          route("embedding", "server", "server_default"),
          route("transcription", "device", "needs_consent"),
        ],
      }),
      serverStatus(),
      status({ location: "on_device" }),
      status({ location: "strict_local" }),
    ]) {
      const html = renderAdvanced(dto);
      expect(html.match(/luna-compute-card__route-switch/g), dto.location).toHaveLength(3);
      expect(html, "「更改」指向了同意开关而不是分配槽").not.toContain(
        `settings://ai_services/${TRANSCRIPTION_CONSENT_KEY}`,
      );
      for (const slot of ["default_llm", "default_embedding", "realtime_asr"]) {
        expect(html, `缺 ${slot} 的入口`).toContain(
          `data-route="settings://ai_services/ai.assignment.${slot}"`,
        );
      }
    }
  });

  /**
   * 一次性交接提示：ADR-037「连上即用」与 ADR-058「显式 pin 被尊重」同时成立时留下的
   * 空档——用户连上了服务端，某几项仍钉在本机，而他不知道。名单由 Rust 算（前端零业务），
   * 前端只负责在名单非空时把这条提示连同两个出口摆出来。
   */
  it("仍钉在本机的那几项会被主动问一次，并给一键切换与保留两个出口（ADR-059）", () => {
    const html = render(
      status({
        location: "organization",
        pending_handover: ["chat", "embedding"],
        routes: [
          route("chat", "device", "explicit_device"),
          route("embedding", "device", "explicit_device"),
          route("transcription", "server", "explicit_server"),
        ],
      }),
    );
    expect(html).toContain(translateForTest("settings:compute_card.handover.title", { count: 2 }));
    expect(html).toContain(translateForTest("settings:compute_card.handover.keep"));
    // 名单为空 = 不提示。这条同样重要：每次打开设置都被问一遍，等于逼用户学会无视它。
    //
    // 判据用**容器类名**而不是按钮文案：「交给服务端」是转写同意项文案
    // 「录音转写也交给服务端」的子串，用文案判的反向断言在任何服务端形态下都恒假——
    // 那是一条永远抓不住回归的死断言（写这条时当场撞上）。
    expect(html).toContain("luna-compute-card__handover");
    const quiet = render(status({ location: "organization", pending_handover: [] }));
    expect(quiet).not.toContain("luna-compute-card__handover");
  });

  it("落点归类不漏：只有两种服务端形态算作「算力出这台设备」", () => {
    const locations = Object.keys(LOCATION_COPY) as ComputeLocation[];
    expect(locations.filter(runsOnServer).sort()).toEqual(["official_cloud", "organization"]);
  });

  /**
   * 已经在本机跑的那条能力没有「本机备用」可言（真机 2026-09-11）。
   * 判据是**这条能力**在不在服务端，不是这台机器的落点在哪。
   */
  it("在本机跑的那条能力不挂本机备用", () => {
    const html = renderAdvanced(
      status({
        location: "official_cloud",
        routes: [
          route("chat", "server", "server_default"),
          route("embedding", "server", "server_default"),
          route("transcription", "device", "explicit_device"),
        ],
      }),
    );
    // 两条在服务端的有备用，转写那条写的是正在用的本机模型。
    expect(html.match(/data-role="fallback"/g)).toHaveLength(2);
    expect(html.match(/data-role="active"/g)).toHaveLength(1);
  });
});
