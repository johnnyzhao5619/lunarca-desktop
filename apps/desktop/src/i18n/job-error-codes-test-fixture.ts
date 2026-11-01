/// <reference types="vite/client" />
/**
 * 测试夹具：从**生成的** `ipc/bindings.ts` 里枚举 `JobErrorCodeDto` 的全部字面量。
 *
 * TS 联合类型在运行期不存在，没法直接枚举；此前各守护用例各手抄一份码表，而手抄只能
 * 被 `satisfies` 守住「抄错 / 后端删了」这一半——**后端新增变体时它不会红**
 * （`translate-failure.test.ts` 因此漏了 4 个码而长期全绿）。
 *
 * 这里改读生成物本身：`bindings.ts` 由 `export-bindings` 从 Rust 枚举导出，CI 会重导出并
 * `git diff --exit-code`，所以它与 `src-tauri/src/ipc/mod.rs` 的 `JobErrorCodeDto` 恒等——
 * 后端多一个变体，这张表就多一个码，靠它的用例当场变红。
 *
 * 只供 `*.test.ts` 引用；生产代码不得 import（它读的是源码文本，不是运行时值）。
 */
import bindingsSource from "../ipc/bindings.ts?raw";
import type { JobErrorCodeDto } from "../ipc/bindings";

/** 生成物里 `JobErrorCodeDto` 联合的全部字面量（顺序同声明）。 */
export function jobErrorCodesFromBindings(): readonly JobErrorCodeDto[] {
  const start = bindingsSource.indexOf("export type JobErrorCodeDto =");
  if (start < 0) {
    throw new Error("bindings.ts 里找不到 `export type JobErrorCodeDto =`——绑定是否重新导出过？");
  }
  const end = bindingsSource.indexOf(";", start);
  const body = bindingsSource
    .slice(start, end)
    // specta 会把变体的 doc comment 夹在字面量之间，先剥掉再取字面量。
    .replace(/\/\*[\s\S]*?\*\//g, "");
  const codes = Array.from(body.matchAll(/"([A-Z][A-Z0-9_]*)"/g), (m) => m[1]!);
  if (codes.length === 0) {
    throw new Error("`JobErrorCodeDto` 联合里一个字面量都没解析出来——生成物格式变了？");
  }
  if (new Set(codes).size !== codes.length) {
    throw new Error(`\`JobErrorCodeDto\` 联合里有重复字面量：${codes.join(", ")}`);
  }
  return codes as JobErrorCodeDto[];
}
