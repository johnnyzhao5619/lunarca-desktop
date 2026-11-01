/**
 * 首帧外观快照的跨语言守护（PRD-14-066 / ADR-114 ④）。
 *
 * Rust 的注入脚本（`src-tauri/src/boot_appearance.rs`）在文档解析前把外观挂到 `<html>`
 * 的 `data-*` 上；React 挂载后 `theme.ts::applyTheme` 用同一套属性名再投影一次。两处各写
 * 一遍是不可避免的（一个跑在解析前、一个跑在 React 里），但属性名一旦分叉，首帧与稳态
 * 就会各挂各的属性——CSS 只认其中一个，于是"闪一下"回来了，而没有任何东西会变红。
 *
 * 这条测试直接读两边的源码，钉住：① 四个 dataset 键 + `--font-scale` 两边都在；
 * ② Rust 快照的字段名与前端 `BootAppearance` 接口一致。
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = resolve(__dirname, "../../../..");
const RUST = readFileSync(resolve(ROOT, "src-tauri/src/boot_appearance.rs"), "utf8");
const THEME = readFileSync(resolve(__dirname, "theme.ts"), "utf8");
const BOOT = readFileSync(resolve(__dirname, "../lib/boot.ts"), "utf8");

describe("首帧外观快照 · Rust 注入脚本与前端映射同源", () => {
  it("五个 data-* 键与 --font-scale 两边都在", () => {
    for (const key of ["theme", "themeFamily", "contrast", "reduceMotion", "density"]) {
      expect(RUST, `Rust 脚本缺 r.dataset.${key}`).toContain(`r.dataset.${key}=`);
      expect(THEME, `theme.ts 缺 root.dataset.${key}`).toContain(`root.dataset.${key} =`);
    }
    expect(RUST).toContain("'--font-scale'");
    expect(THEME).toContain('"--font-scale"');
  });

  it("快照字段名与前端 BootAppearance 接口一致", () => {
    const rustStruct = RUST.slice(RUST.indexOf("pub struct BootAppearance"), RUST.indexOf("/// 与前端"));
    const rustFields = [...rustStruct.matchAll(/pub (\w+):/g)].map((m) => m[1]).sort();
    const tsInterface = BOOT.slice(BOOT.indexOf("export interface BootAppearance"), BOOT.indexOf("declare global"));
    const tsFields = [...tsInterface.matchAll(/^\s+(\w+):/gm)].map((m) => m[1]).sort();
    expect(rustFields.length).toBeGreaterThan(0);
    expect(tsFields).toEqual(rustFields);
  });
});
