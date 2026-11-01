/// <reference types="vite/client" />
/**
 * 「回车即动作」的键盘处理必须经共享输入法判据（PRD-07-040 ①、TDD-07 §4.2）。
 *
 * 2026-10-04 盘点时有十几处输入框只判 `event.key === "Enter"`（或只判
 * `nativeEvent.isComposing`），WKWebView 里中文输入法上屏那一下就被当成了提交。
 * 本守护按文件判：源码里出现回车判定的文件，必须**导入**共享判据
 * （`lib/ime-key-event` 或 `lib/ime-composition`），否则必须在下面的白名单里写明理由。
 *
 * 它证明的是「这份文件接入了判据」，不是「每一处回车都问过它」——同一文件里新增第二处
 * 回车而忘了问，本守护看不出来；那一层由各面的行为用例负责。
 */
import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob(["../**/*.{ts,tsx}", "!../**/*.test.{ts,tsx}"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 不经判据也正确的回车处理：不是文本输入框，或必须带修饰键。 */
const NOT_A_TEXT_INPUT: Record<string, string> = {
  "../features/editor/FloatingMenu.tsx": "只认 ⌘/Ctrl+Enter，上屏的回车不带修饰键",
  "../features/voice-input/VoiceInputHud.tsx": "只认 ⌘/Ctrl+Enter",
  "../features/onboarding/OnboardingWizard.tsx": "卡片级回车，输入框里的回车由 ENTER_OWNED_BY 让给输入框自己",
  "../features/playback/SyncPlaybackView.tsx":
    "字幕清单的方向键 / 回车不在输入框里；查找框经 QueryBox，判据在 QueryBox 内",
  "../features/popup-translate/ScreenshotOverlay.tsx": "框选覆盖层，没有输入框",
  "../hooks/use-status-visit-metrics.ts": "状态栏按钮的键盘激活，没有输入框",
};

const ENTER_CHECK = /\bkey\s*[!=]==\s*"Enter"|case\s+"Enter"/;
const IMPORTS_RULE = /^import[^;]*from\s+"[^"]*lib\/ime-(?:key-event|composition)";/m;

/** 违规 = 判了回车、没导入判据、也不在白名单里。 */
function enterHandlersWithoutImeRule(
  sources: Record<string, string>,
  allow: Record<string, string>,
): string[] {
  return Object.entries(sources)
    .filter(([path, source]) => ENTER_CHECK.test(source) && !IMPORTS_RULE.test(source) && !(path in allow))
    .map(([path]) => path)
    .sort();
}

describe("回车即动作的键盘处理经共享输入法判据", () => {
  it("全部源码：没有遗漏", () => {
    expect(enterHandlersWithoutImeRule(SOURCES, NOT_A_TEXT_INPUT)).toEqual([]);
  });

  it("白名单里的每一条都真实存在且确实判了回车（陈旧条目会让守护悄悄放过同名新文件）", () => {
    for (const path of Object.keys(NOT_A_TEXT_INPUT)) {
      expect(SOURCES[path], path).toBeDefined();
      expect(ENTER_CHECK.test(SOURCES[path] ?? ""), path).toBe(true);
    }
  });

  it("反向夹具：只判回车的输入框会被抓出来", () => {
    const fixture = {
      "a.tsx": 'onKeyDown={(event) => { if (event.key === "Enter") submit(); }}',
      "b.tsx": 'onKeyDown={(event) => { if (event.nativeEvent.isComposing) return; switch (event.key) { case "Enter": go(); } }}',
    };
    expect(enterHandlersWithoutImeRule(fixture, {})).toEqual(["a.tsx", "b.tsx"]);
  });

  it("反向夹具：注释里提到判据不算接入", () => {
    const fixture = {
      "c.tsx": '// 见 lib/ime-key-event\nonKeyDown={(e) => { if (e.key === "Enter") submit(); }}',
    };
    expect(enterHandlersWithoutImeRule(fixture, {})).toEqual(["c.tsx"]);
  });

  it("正向夹具：导入了判据即放行", () => {
    const fixture = {
      "d.tsx":
        'import { isImeKeyEvent } from "../../lib/ime-key-event";\nonKeyDown={(e) => { if (isImeKeyEvent(e)) return; if (e.key === "Enter") submit(); }}',
    };
    expect(enterHandlersWithoutImeRule(fixture, {})).toEqual([]);
  });
});
