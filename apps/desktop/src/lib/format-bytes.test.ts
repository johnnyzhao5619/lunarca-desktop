/**
 * 字节量格式化——**全应用唯一一份**（用量面板的 `*_bytes` 维度、模型面板、语音 HUD、
 * 首启引导、回收站、数据足迹、下载进度共用）。
 *
 * 前几条钉的是一次真机缺陷的后半段：出站额度是 `5,368,709,120`，此前原样渲染成一串
 * 裸数字——既读不出量级，也无法与运营口中的额度对上。
 *
 * 末尾两条钉的是 2026-09-09 的收口：此前 `lib/format-bytes.ts` 与 `lib/format-datetime.ts`
 * 各有一份实现，**都按 1024 换算**却一份标 `GB`、一份标 `GiB`，于是同一个数字在
 * 「数据足迹」与「用量记录」两页读法不同。合并保留 IEC 那一份的口径，同时保留另一份
 * 「体积不可知 → `—`」的语义——它是被合并掉的那份唯一带着、且不能丢的行为。
 */

import { describe, expect, it } from "vitest";
import { formatBytes, isByteDimension } from "./format-bytes";

describe("formatBytes", () => {
  /** 服务端那个额度常量是 2³⁰×5，必须整整齐齐地读回「5 GiB」。 */
  it("renders the real egress quota as a round IEC value", () => {
    expect(formatBytes(5_368_709_120, "en-US")).toBe("5\u00a0GiB");
    expect(formatBytes(5_368_709_120, "zh-CN")).toBe("5\u00a0GiB");
  });

  /** 三位有效数字：跨量级读起来一致，且不会给出假精度。 */
  it("keeps three significant digits once scaled", () => {
    expect(formatBytes(1_024_591, "en-US")).toBe("1,000\u00a0KiB");
    expect(formatBytes(10_240_000, "en-US")).toBe("9.77\u00a0MiB");
    expect(formatBytes(536_870_912, "en-US")).toBe("512\u00a0MiB");
  });

  /** 字节本身不给小数——「1.00 B」是噪音。 */
  it("does not fabricate decimals below one KiB", () => {
    expect(formatBytes(0, "en-US")).toBe("0\u00a0B");
    expect(formatBytes(999, "en-US")).toBe("999\u00a0B");
  });

  /**
   * 数字与单位之间必须有空格（`128 GiB` 而非 `128GiB`，三家风格指南一致），
   * 而且必须是**不换行**空格：普通空格会让「5」和「GiB」被折到两行，
   * 那在窄栏里是必然发生的事，不是理论风险。
   */
  it("separates value and unit with a non-breaking space", () => {
    expect(formatBytes(2048, "en-US")).toBe("2\u00a0KiB");
    expect(formatBytes(2048, "en-US")).not.toContain(" ");
  });

  /**
   * **「不知道多大」不得渲染成 `0 B`，更不得渲染成 `NaN`**（合并前 `format-bytes` 那份
   * 的语义，必须原样保住）：模型面板上「未知占用」被读成「不占空间」，会让人以为删了
   * 也白删。占位是 em dash，不是 0。
   */
  it("renders an em dash when the size is unknown, never NaN and never a fake zero", () => {
    expect(formatBytes(null, "en-US")).toBe("—");
    expect(formatBytes(undefined, "en-US")).toBe("—");
    expect(formatBytes(Number.NaN, "en-US")).toBe("—");
    expect(formatBytes(Number.POSITIVE_INFINITY, "en-US")).toBe("—");
  });

  /**
   * **反过来，0 也不能写成「—」**：回收站空着时「占用 0 B」是个事实，「占用 —」
   * 看起来像出了故障。合并前的 `format-bytes` 把 `<= 0` 一并吞成占位，那是把两件事
   * 混成了一件。
   */
  it("keeps zero and negative deltas as real numbers", () => {
    expect(formatBytes(0, "zh-CN")).toBe("0\u00a0B");
    expect(formatBytes(-1536, "en-US")).toBe("-1.5\u00a0KiB");
  });

  /** 数值部分走界面语言：不传 locale 读的是系统语言（TDD-11 §1.4b）。 */
  it("localizes the numeric part with the injected locale", () => {
    expect(formatBytes(1_536_000, "de-DE")).toBe("1,46\u00a0MiB");
    expect(formatBytes(1_536_000, "en-US")).toBe("1.46\u00a0MiB");
  });
});

describe("isByteDimension", () => {
  it("matches the byte dimensions and nothing else", () => {
    expect(isByteDimension("egress_bytes")).toBe(true);
    expect(isByteDimension("tokens_in")).toBe(false);
    expect(isByteDimension("requests")).toBe(false);
  });

  /** `storage_byte_hours` 的量纲是「字节 × 小时」——按字节渲染就是个单位错误的数。 */
  it("leaves byte-hours alone", () => {
    expect(isByteDimension("storage_byte_hours")).toBe(false);
  });
});
