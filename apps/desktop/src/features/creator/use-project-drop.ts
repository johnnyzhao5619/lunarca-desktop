/**
 * 一块接收「拖进项目」的区域（ADR-167 ③）：挂上 `data-project-drop`（由 `use-media-drop` 判落点）
 * 并在这里接住派发来的那一次拖放。项目概览与「添加来源」的文件页各挂一块。
 */

import { useEffect, useRef, type RefObject } from "react";

import { PROJECT_FILE_DROP_EVENT, type ProjectFileDrop } from "../../lib/project-file-drop";

export function useProjectDrop<T extends HTMLElement>(onPaths: (paths: string[]) => void): RefObject<T> {
  const ref = useRef<T>(null);
  const onPathsRef = useRef(onPaths);
  onPathsRef.current = onPaths;
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const listener = (event: Event) => onPathsRef.current((event as CustomEvent<ProjectFileDrop>).detail.paths);
    element.addEventListener(PROJECT_FILE_DROP_EVENT, listener);
    return () => element.removeEventListener(PROJECT_FILE_DROP_EVENT, listener);
  });
  return ref;
}
