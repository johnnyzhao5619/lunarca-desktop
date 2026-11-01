import { useState } from "react";
import { commands } from "../../ipc/bindings";
import { useTaskStartGate } from "../../hooks/use-task-start-gate";

/**
 * 媒体导入动作（PRD-05-030）。
 *
 * ADR-003 前端零业务：文件多选对话框、格式过滤、模型可用性早验证、入队与顺序处理
 * 全在 Rust（`pick_media_files` 选、`import_media_paths` 入队——与拖入同一条）；此处只持「进行中」
 * 这一 UI 态，在选与入队之间过一道任务前预估（PRD-13-040 ②：服务端转写时不够就当场说清、不入队），
 * 并把结果回流给调用方。
 *
 * `enqueue` 决定选好的文件排进哪里：缺省是「转写」页那条（转写笔记建在默认的转写文件夹）；
 * 创作者发布包页传 `creatorImportFinalCut`，转写笔记直接建进项目文件夹（ADR-162 ⑪）。
 * `pick` 决定选择框认哪些文件：缺省只认音视频；项目概览「导入素材…」传 `pickMaterialFiles`，
 * 音视频与文档都能选（ADR-164 ③，分流在 Rust）。
 */
export interface TranscribeImport {
  /** 对话框已打开、命令未返回（按钮禁用与文案切换用）。 */
  importing: boolean;
  /** 唤起导入；返回入队件数（0 = 用户取消、无有效文件或预估说不够）。失败经 `onError` 回流。 */
  start: () => Promise<number>;
}

export function useTranscribeImport(
  onError: (message: string) => void,
  enqueue: typeof commands.importMediaPaths = commands.importMediaPaths,
  pick: typeof commands.pickMediaFiles = commands.pickMediaFiles,
): TranscribeImport {
  const [importing, setImporting] = useState(false);
  const gate = useTaskStartGate();

  async function start(): Promise<number> {
    if (importing) {
      return 0;
    }
    setImporting(true);
    try {
      const picked = await pick();
      if (picked.status === "error") {
        onError(picked.error);
        return 0;
      }
      const paths = picked.data;
      if (paths.length === 0 || !(await gate({ kind: "import_media", paths }))) {
        return 0;
      }
      const result = await enqueue(paths);
      if (result.status === "error") {
        onError(result.error);
        return 0;
      }
      return result.data;
    } finally {
      setImporting(false);
    }
  }

  return { importing, start };
}
