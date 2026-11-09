import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { commands } from "../../ipc/bindings";
import { useTaskStartGate } from "../../hooks/use-task-start-gate";
import { dropOnEditor, editorSurfaceAt, logicalPoint, markEditorDropTarget } from "../../lib/editor-file-drop";
import { dropOnProject, markProjectDropTarget, projectDropAt } from "../../lib/project-file-drop";
import {
  dropOnTeleprompter,
  markTeleprompterDropTarget,
  teleprompterDropAt,
} from "../../lib/teleprompter-file-drop";
import { dropOnTree, markTreeDropTarget, treeDropAt } from "../../lib/tree-file-drop";

/**
 * 从系统拖入音/视频文件即入队转写（PRD-05-001 / PRD-07-008）；落点在编辑器正文上时改为
 * 交给那个编辑器插成附件（对标方案 B4，`lib/editor-file-drop.ts`），落点在文件树上时改为
 * 复制进那个文件夹（W5a，`lib/tree-file-drop.ts`），落点在创作项目上时作为项目来源导入
 * （ADR-167 ③，`lib/project-file-drop.ts`），落点在提词器首页上时转成提词稿（ADR-176 ⑨，
 * `lib/teleprompter-file-drop.ts`）。
 *
 * 依赖 `dragDropEnabled: true`（`tauri.conf.json`）——它与 webview 的 HTML5 DnD 互斥，
 * 故应用内的树/标签拖拽已改用指针事件（`lib/pointer-drag.ts`），二者不再冲突。
 *
 * ADR-003 前端零业务：这里只把 OS 给的绝对路径原样转发给 Rust，**筛选与校验在 Rust**
 * （`import_media_paths` 按可播放媒体扩展名白名单过滤）。前端连"这是不是媒体文件"都不猜。
 */
export interface MediaDrop {
  /** 是否有文件正悬停在窗口上（驱动放置态高亮）。 */
  hovering: boolean;
}

export function useMediaDrop(
  onImported: (count: number) => void,
  onError: (message: string) => void,
): MediaDrop {
  const [hovering, setHovering] = useState(false);
  // 回调经 ref 固定：订阅只建一次（重复订阅会让一次拖放被处理多次），
  // 又不必要求调用方保证引用稳定——依赖数组是真的空，不靠注释豁免。
  const onImportedRef = useRef(onImported);
  onImportedRef.current = onImported;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  // 服务端转写时先问一句够不够（PRD-13-040 ②）：同样经 ref 固定，订阅只建一次。
  const gate = useTaskStartGate();
  const gateRef = useRef(gate);
  gateRef.current = gate;
  const webviewHeight = useRef<number | undefined>(undefined);

  useEffect(() => {
    let disposed = false;
    const pending = getCurrentWebview().onDragDropEvent(async (event) => {
      if (disposed) {
        return;
      }
      // 框高在拖入时与松手时各取一次（窗口可能在两次拖放之间缩放过），用来校正标题栏那一截。
      if (event.payload.type === "enter" || event.payload.type === "drop") {
        webviewHeight.current = (await getCurrentWebview().size()).height;
      }
      // 落点在编辑器正文上：文件插进那篇笔记当附件（B4），不当作媒体导入。
      const point =
        "position" in event.payload ? logicalPoint(event.payload.position, webviewHeight.current) : null;
      const surface = point ? editorSurfaceAt(point.x, point.y) : null;
      const tree = point ? treeDropAt(point.x, point.y) : null;
      const project = point ? projectDropAt(point.x, point.y) : null;
      const teleprompter = point ? teleprompterDropAt(point.x, point.y) : null;
      if (event.payload.type === "enter" || event.payload.type === "over") {
        markEditorDropTarget(surface);
        markTreeDropTarget(tree);
        markProjectDropTarget(project);
        markTeleprompterDropTarget(teleprompter);
        setHovering(surface === null && tree === null && project === null && teleprompter === null);
        return;
      }
      markEditorDropTarget(null);
      markTreeDropTarget(null);
      markProjectDropTarget(null);
      markTeleprompterDropTarget(null);
      setHovering(false);
      if (event.payload.type !== "drop") {
        return;
      }
      if (point && dropOnEditor("system", event.payload.paths, point.x, point.y)) {
        return;
      }
      // 落点在文件树上：复制进那个文件夹（W5a），不当作媒体导入。
      if (point && dropOnTree(event.payload.paths, point.x, point.y)) {
        return;
      }
      // 落点在创作项目上：作为这个项目的来源导入（ADR-167 ③），不进「转写」。
      if (point && dropOnProject(event.payload.paths, point.x, point.y)) {
        return;
      }
      // 落点在提词器首页上：转成提词稿并打开（ADR-176 ⑨），不进「转写」。
      if (point && dropOnTeleprompter(event.payload.paths, point.x, point.y)) {
        return;
      }
      // 预估不够时当场说清、不入队；够、在本机跑、拖进来的不是媒体（Rust 判）都照常往下走。
      const paths = event.payload.paths;
      if (!(await gateRef.current({ kind: "import_media", paths }))) {
        return;
      }
      const result = await commands.importMediaPaths(paths);
      if (result.status === "error") {
        onErrorRef.current(result.error);
        return;
      }
      // 0 = 拖进来的全不是音/视频（用户可能连同文档一起拖了），静默即可。
      if (result.data > 0) {
        onImportedRef.current(result.data);
      }
    });
    return () => {
      disposed = true;
      void pending.then((dispose) => dispose());
    };
  }, []);

  return { hovering };
}
