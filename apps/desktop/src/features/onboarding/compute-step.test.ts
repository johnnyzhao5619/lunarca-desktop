/**
 * 首启引导「登录（可跳过）」的文案守护。
 *
 * `OnboardingWizard` 对**每一步**无条件渲染 `step.<id>.title` / `body`。删掉 key 时它不会
 * 报错，只会在屏幕上显示一行裸 key（真机 2026-08-09 截图坐实）。
 */

import { describe, expect, it } from "vitest";

import en from "../../i18n/locales/en-US/onboarding.json";
import zh from "../../i18n/locales/zh-CN/onboarding.json";

describe("引导 · 登录（可跳过）", () => {
  it("向导要求的文案在中英资源包里都在场", () => {
    for (const bundle of [zh, en]) {
      const compute = bundle.step.compute as Record<string, unknown>;
      for (const key of ["title", "body", "loading", "skip_hint", "signed_in_as"]) {
        expect(compute[key], `缺 step.compute.${key}`).toBeTypeOf("string");
      }
    }
  });
});
