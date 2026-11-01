import { describe, expect, it } from "vitest";
import zhCommon from "./locales/zh-CN/common.json";
import enCommon from "./locales/en-US/common.json";
import zhTranscribe from "./locales/zh-CN/transcribe.json";
import enTranscribe from "./locales/en-US/transcribe.json";
import zhRecording from "./locales/zh-CN/recording.json";
import enRecording from "./locales/en-US/recording.json";
import { i18n } from "./i18n";
import { failureMessage } from "./failure";
import { fetchErrorKey } from "../lib/task-failure";
import { jobErrorCodesFromBindings } from "./job-error-codes-test-fixture";
import type { JobErrorCodeDto } from "../ipc/bindings";

/**
 * **每个 Job 错误码，在它能到达的每条渲染通道上，都必须有中英文案；到不了的通道上
 * 不许有**（TDD-15 §4.3 通道归属判据 / §5 L10）。
 *
 * 全集来自生成物 `bindings.ts`（`job-error-codes-test-fixture.ts`），不再手抄——后端新增
 * 变体、这里没登记归属，`typecheck`（`Record<JobErrorCodeDto, …>` 缺键）与 `vitest`
 * （全集 ≠ 登记键集）都会红。
 *
 * **批 E2（ADR-111 ④）改写了通道拓扑**：
 * - 原 `translate` / `chat` 两条 LLM 通道**并为一条 `llm`**——对话、翻译弹窗、笔记 AI
 *   悬浮菜单、语音 HUD 的错误码全部渲染自唯一注册表 `common:failure.*`（`i18n/failure.ts`）；
 *   同一个码在这些表面上从此只有一句话，表面差异只剩后果从句（`FailureCard.consequence`）。
 * - `voice-input` 通道保留为**归属声明**（哪些码的生产者在语音路径上），但它的键面也是
 *   `common:failure.*`。ADR-109 ④「判据从 VoiceInputHud 的 switch 现读」随之退役：
 *   那条判据守的失败模式是「switch 少接一个码 → 中性兜底」，而 switch 已被通用映射取代
 *   （`errors:<code>` → `failureMessage`），少接一个码这件事在结构上不可能发生了。
 * - 转写族三条通道原样保留：`transcribe`（批式）与 `recording` / `recording-translate`
 *   （流式掉线横幅）说的是「非阻断降级 + 后果」，与「这一轮没开始」语义确实不同。
 *
 * **共享键面的死文案判据**：`llm` 与 `voice-input` 共用 `common:failure.*`，故某码的
 * 文案允许存在 ⟺ 它归属于两者**至少其一**；两者都不归属而文案在，即死文案（例：给
 * `QUEUE_EXPIRED` 写一句 common 文案 → 红）。
 *
 * 反向验证过：删掉任一通道的任一条文案 → 红；给 `common:failure.QUEUE_EXPIRED` 加一句
 * → 红；从 `HOME` 里删掉一个码 → typecheck 与 vitest 都红。
 */

type ChannelName = "llm" | "transcribe" | "recording" | "recording-translate" | "voice-input";

type Bundle = Record<string, unknown>;

interface Channel {
  /** 渲染这条通道的代码路径（供报错时定位）。 */
  surface: string;
  /** 该表面为这个码实际取到的 key（`ns:path`），`null` = 落到通用兜底 / 根本不认识它。 */
  keyFor(code: JobErrorCodeDto): string | null;
  bundles: { zh: Bundle; en: Bundle };
  /** 与谁共用键面（死文案按共用组的归属并集判定）。 */
  registry: "common-failure" | "own";
}

/**
 * 只有 ASR 路径能产出的码 → 值 = 生产者位置。这是「llm 通道豁免」的**证据**：改了生产者
 * （比如某天 LLM 路径也开始回 `QUEUE_EXPIRED`）就得同步改这里，然后 llm 通道会要求补文案。
 */
const ASR_ONLY_CODES = {
  QUEUE_EXPIRED: "crates/luna-provider/src/asr_remote.rs（服务端批式转写 `expired` 终态，ADR-049 ⑦）",
  RESULT_UNAVAILABLE: "crates/luna-provider/src/asr_remote.rs（`finished` 上取不回 segments）",
  LOCAL_SESSION_BUSY: "crates/luna-jobqueue/src/queue.rs（流式会话池准入到期，ADR-078 ①/③）",
} as const satisfies Partial<Record<JobErrorCodeDto, string>>;

/**
 * 逐码归属表。`Record<JobErrorCodeDto, …>`：后端删码 → 多余键红；后端加码 → 缺键红。
 *
 * - `recording`（实时字幕 HUD 的 `transcribe_stopped.<code>`）只收流式 ASR 会掉线的那些码，
 *   其余码那一面走 `transcribe_stopped.generic`——「实时字幕已停，录音继续」对它们已足够。
 * - `transcribe`（队列面板 / 回放的 `taskFailureText`）只收批式转写路径上**需要用户自己
 *   决定下一步**的码；其余码那一面走 `common:failure.generic`。
 * - `voice-input`：语音 HUD 能收到的码（Rust `voice_error_message_key` 只在语音 Job 的
 *   失败路径上发）。它的存在证成 `LOCAL_SESSION_BUSY` 在注册表里的那句文案。
 */
const HOME: Record<JobErrorCodeDto, readonly ChannelName[]> = {
  MODEL_MISSING: ["llm", "recording", "recording-translate", "voice-input"],
  PERMISSION_DENIED: ["llm", "recording-translate"],
  // 账户会话被拒（TDD-01 §2.4）：云端对话、两条转写腿与实时转译都用同一份会话。
  SIGN_IN_REQUIRED: ["llm", "recording", "recording-translate"],
  PROVIDER_NOT_CONFIGURED: ["llm"],
  RESOURCE_EXHAUSTED: ["llm"],
  QUOTA_EXHAUSTED: ["llm", "recording", "recording-translate"],
  // 组织没开放（R4b）：有本机腿时路由已回落，只有一条本机腿都没有的对话 / 实时转译会见到它；
  // 实时转写恒有随包 whisper 接手（`asr_failover`），故不登记 `recording`。
  CAPABILITY_NOT_ALLOWED: ["llm", "recording-translate"],
  // 账户被冻结（纵切 3 C3）：对话、两条转写腿与实时转译共用同一个账户；结构性，故登记
  // `recording-translate`；实时转写不回落本机（同额度用尽），故登记 `recording`。
  ACCOUNT_FROZEN: ["llm", "recording", "recording-translate"],
  // 撞上自设的本月预付上限（纵切 3 C4）：同账户、同处置——结构性，故登记 `recording-translate`；
  // 实时转写在下一个计量窗口撞上它时停云腿、不回落本机（同额度用尽），故登记 `recording`。
  SPEND_CAP_REACHED: ["llm", "recording", "recording-translate"],
  // 领不到免费额度（纵切 3 C5）：同账户、同处置——结构性，故登记 `recording-translate`；实时转写的
  // 握手 402 / 错误帧停云腿、不回落本机（同额度用尽），故登记 `recording`。
  FREE_QUOTA_WITHHELD: ["llm", "recording", "recording-translate"],
  RATE_LIMITED: ["llm", "recording", "voice-input"],
  ALREADY_PROCESSED: ["llm"],
  PROVIDER_UNAVAILABLE: ["llm", "recording", "voice-input"],
  QUEUE_EXPIRED: ["transcribe"],
  RESULT_UNAVAILABLE: ["transcribe"],
  STRICT_LOCAL_BLOCKED: ["llm", "recording", "transcribe", "recording-translate"],
  // **不含 `recording` / `transcribe`**：服务端只对**模型形**泳道（chat / translate /
  // summarize / refine / embed）回这个码；两条转写通道结构性为空时仍回 `upstream_unavailable`
  // （服务端 `EmptyChain::into_error` 里那两支各有自己的 注释说明为什么本批不动它）。
  ROUTE_LANE_EMPTY: ["llm", "recording-translate"],
  LOCAL_SESSION_BUSY: ["recording", "voice-input"],
  TIMEOUT: ["llm", "recording", "voice-input"],
  CANCELLED: ["llm"],
  INVALID_INPUT: ["llm", "voice-input"],
  // 服务端背后那一层出了问题（`upstream_unavailable` / `internal`，ADR-097）。
  // **非结构性**（上游会恢复）→ 逐句消费者继续下一句，故不登记 `recording-translate`。
  SERVER_SIDE_FAILURE: ["llm"],
  // 服务端这台部署配错了（`egress_blocked` / `upstream_protocol`）。**结构性**：
  // 换下一句也一样，实时转译腿据此一次性升格为会话级横幅而不是逐句失败一百次。
  SERVER_MISCONFIGURED: ["llm", "recording-translate"],
  // 服务端选中的模型只吐推理、正文为空（ADR-102）。**非结构性**——每次受理都重新挑
  // 一次上游，下一句未必空——故不登记 `recording-translate`。队列面板扫的就是
  // `common:failure.*`，即 llm 通道这条。
  EMPTY_AFTER_THINKING: ["llm"],
  // 本机推理引擎如实拒绝超窗输入（ADR-143）。**非结构性**——下一句未必这么长——
  // 故不登记 `recording-translate`；实时转译的一句话也远到不了窗口。
  INPUT_TOO_LONG: ["llm"],
  UPSTREAM: ["llm", "voice-input"],
  COMMAND_FAILED: ["llm"],
};

const CHANNELS: Record<ChannelName, Channel> = {
  llm: {
    surface:
      "i18n/failure.ts `failureMessage`（弹窗翻译 / 笔记 AI 悬浮菜单 / 对话与翻译槽的 FailureCard，ADR-111 ④ 唯一注册表）",
    keyFor: (code) => `common:failure.${code}`,
    bundles: { zh: zhCommon, en: enCommon },
    registry: "common-failure",
  },
  transcribe: {
    surface: "lib/task-failure.ts `fetchErrorKey`（队列面板 / 回放的 `taskFailureText`）",
    keyFor: (code) => {
      const key = fetchErrorKey(code);
      return key === null ? null : `transcribe:${key}`;
    },
    bundles: { zh: zhTranscribe, en: enTranscribe },
    registry: "own",
  },
  recording: {
    surface:
      "features/recording/RecordingHud.tsx · features/caption/CaptionPanel.tsx（`transcribe_stopped.${code.toLowerCase()}`）",
    // 组件里是模板串，没法 import；这里复述那条公式，两边都改才对得上（`generic` 是兜底）。
    keyFor: (code) => `recording:transcribe_stopped.${code.toLowerCase()}`,
    bundles: { zh: zhRecording, en: enRecording },
    registry: "own",
  },
  // 与 `recording` 分成两条通道，不是同一条（ADR-083 ⑧）：转写腿停了与转译腿停了是
  // 两句不同的话（「字幕不出字」vs「原文照常上屏、只是没有译文」），下一步动作也不同。
  // 归属的码 = `JobError::is_structural()` 为真的那些。
  "recording-translate": {
    surface:
      "features/recording/RecordingHud.tsx · features/caption/CaptionPanel.tsx（`translate_stopped.${code.toLowerCase()}`）",
    keyFor: (code) => `recording:translate_stopped.${code.toLowerCase()}`,
    bundles: { zh: zhRecording, en: enRecording },
    registry: "own",
  },
  // 键由 Rust 现拼（`voice_error_message_key` 发 `errors:<code 小写>`），前端通用映射到
  // `failureMessage`；认不出的码回中性兜底。本通道的归属声明证成注册表里语音专属码的文案。
  "voice-input": {
    surface:
      "src-tauri/src/voice_input.rs `voice_error_message_key` → features/voice-input/voice-message.ts `keyToMessage`（errors:* 通用映射；语音 HUD 与翻译窗共用）",
    keyFor: (code) => `common:failure.${code}`,
    bundles: { zh: zhCommon, en: enCommon },
    registry: "common-failure",
  },
};

const COMMON_REGISTRY_CHANNELS = (
  Object.entries(CHANNELS) as [ChannelName, Channel][]
)
  .filter(([, channel]) => channel.registry === "common-failure")
  .map(([name]) => name);

/** 按 `ns:a.b.c` 从资源包里逐层取值。 */
function lookup(bundle: Bundle, key: string): unknown {
  const path = key.slice(key.indexOf(":") + 1).split(".");
  return path.reduce<unknown>((node, seg) => (node as Bundle | undefined)?.[seg], bundle);
}

const ALL_CODES = jobErrorCodesFromBindings();

function homedInCommonRegistry(code: JobErrorCodeDto): boolean {
  return COMMON_REGISTRY_CHANNELS.some((name) => HOME[code].includes(name));
}

describe("Job 错误码 ↔ 渲染通道 ↔ 中英文案（TDD-15 §4.3 / §5 L10）", () => {
  it("归属表登记的码 = 生成物 bindings.ts 里的全集（后端加码 / 删码这里都红）", () => {
    expect(new Set(Object.keys(HOME))).toEqual(new Set(ALL_CODES));
    for (const code of Object.keys(ASR_ONLY_CODES)) {
      expect(ALL_CODES, `ASR_ONLY_CODES 里的 ${code} 不在 JobErrorCodeDto 全集里`).toContain(code);
    }
  });

  it("每个码至少有一条通道能把它说成人话", () => {
    for (const code of ALL_CODES) {
      const home: readonly ChannelName[] | undefined = HOME[code];
      expect(home, `${code} 未登记通道归属（后端新增了变体？）`).toBeDefined();
      expect(home?.length, `${code} 没有任何渲染通道——它会在所有表面上落成通用兜底`).toBeGreaterThan(0);
    }
  });

  it("llm 通道收下每一个码，只豁免生产者仅在 ASR 路径上的那几个", () => {
    for (const code of ALL_CODES) {
      const asrOnly = code in ASR_ONLY_CODES;
      expect(
        HOME[code].includes("llm"),
        asrOnly
          ? `${code} 只有 ASR 生产者（${ASR_ONLY_CODES[code as keyof typeof ASR_ONLY_CODES]}），不该登记到 llm`
          : `${code} 没有登记为 ASR 专属，就必须在 llm 通道上有文案`,
      ).toBe(!asrOnly);
    }
  });

  it("归属的通道上中英文案都在，且真实取键函数取到的正是它（不是兜底）", () => {
    for (const code of ALL_CODES) {
      for (const name of HOME[code]) {
        const channel = CHANNELS[name];
        const key = channel.keyFor(code);
        expect(key, `${code} 在 ${name} 通道（${channel.surface}）上取不到专属 key`).not.toBeNull();
        expect(lookup(channel.bundles.zh, key!), `zh 缺 ${key}（${code} @ ${name}）`).toBeTruthy();
        expect(lookup(channel.bundles.en, key!), `en 缺 ${key}（${code} @ ${name}）`).toBeTruthy();
      }
    }
  });

  it("没归属的通道上不许有这个码的文案（共享键面按归属并集判死文案）", () => {
    for (const code of ALL_CODES) {
      // 独享键面的通道：逐通道判。
      for (const [name, channel] of Object.entries(CHANNELS) as [ChannelName, Channel][]) {
        if (channel.registry !== "own") continue;
        if (HOME[code].includes(name)) continue;
        const key = channel.keyFor(code);
        if (key === null) continue;
        for (const [locale, bundle] of Object.entries(channel.bundles)) {
          expect(
            lookup(bundle, key),
            `${locale} 里有 ${key}，但 ${code} 到不了 ${name} 通道——要么补 HOME 归属（并说明生产者），要么删文案`,
          ).toBeUndefined();
        }
      }
      // 共享 common:failure 键面：llm 与 voice-input 都不归属才算死文案。
      if (!homedInCommonRegistry(code)) {
        for (const [locale, bundle] of [
          ["zh", zhCommon],
          ["en", enCommon],
        ] as const) {
          expect(
            lookup(bundle, `common:failure.${code}`),
            `${locale} 里有 common:failure.${code}，但它到不了任何共享注册表的表面——死文案`,
          ).toBeUndefined();
        }
      }
    }
  });

  it("注册表运行时函数对每个归属码给出专属句子，其余码回 failure.generic", async () => {
    for (const lng of ["zh-CN", "en-US"] as const) {
      await i18n.changeLanguage(lng);
      const generic = i18n.t("common:failure.generic");
      for (const code of ALL_CODES) {
        const message = failureMessage(code);
        if (homedInCommonRegistry(code)) {
          expect(message, `${lng}: ${code} 在注册表通道落成了通用兜底`).not.toBe(generic);
        } else {
          expect(message, `${lng}: ${code} 不属于任何注册表表面却有专属句子（死文案）`).toBe(generic);
        }
      }
    }
    await i18n.changeLanguage("zh-CN");
  });
});
