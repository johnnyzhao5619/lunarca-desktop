/**
 * 文件树的排序方式与展开的文件夹（PRD-06-006 排序「记住」；ADR-140 设备状态）。
 *
 * 此前两者都是组件内状态，重启即回到「按名称、全部收起」。现在由 Rust 按库身份存在应用
 * 配置目录（不进库、不进用户的 git），这里只做取数与回写——剪枝（已删的文件夹、越界路径）
 * 在 Rust 读出时做（ADR-003）。
 *
 * 三条时序约定：
 * - **读回之前不写**：否则挂载那一刻的空状态会把已存的状态盖掉；
 * - **读回晚到不丢东西**：展开集合与此刻已展开的**取并集**——挂载那一刻文件树会自动展开
 *   当前笔记所在的文件夹，若把它当成「用户动过」而丢弃读回，其余展开着的文件夹就全丢了
 *   （V3b 真机发现）；排序只在用户真改过时以用户为准；
 * - **写入串行、每次发最新的快照**：Tauri 命令并发执行，两次快速的写入可能后发先至，
 *   最后落盘的就成了旧状态。
 */

import { useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";

import { commands, type TreeSortDto, type TreeViewStateDto } from "../../ipc/bindings";

export interface TreeViewState {
  expandedDirs: Set<string>;
  setExpandedDirs: Dispatch<SetStateAction<Set<string>>>;
  sortMode: TreeSortDto;
  setSortMode: (mode: TreeSortDto) => void;
}

export function useTreeViewState(): TreeViewState {
  const [expandedDirs, setExpandedState] = useState<Set<string>>(() => new Set());
  const [sortMode, setSortState] = useState<TreeSortDto>("name");
  const [restored, setRestored] = useState(false);
  const sortTouched = useRef(false);
  const latest = useRef<TreeViewStateDto>({ sort: "name", expanded: [] });
  const saving = useRef<Promise<unknown>>(Promise.resolve());

  useEffect(() => {
    let disposed = false;
    void commands.treeViewState().then((result) => {
      if (disposed) return;
      if (result.status === "ok") {
        setExpandedState((current) => new Set([...result.data.expanded, ...current]));
        if (!sortTouched.current) setSortState(result.data.sort);
      }
      setRestored(true);
    });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    if (!restored) return;
    latest.current = { sort: sortMode, expanded: [...expandedDirs] };
    saving.current = saving.current.then(() => commands.setTreeViewState(latest.current));
  }, [restored, sortMode, expandedDirs]);

  const setExpandedDirs: Dispatch<SetStateAction<Set<string>>> = setExpandedState;
  const setSortMode = (mode: TreeSortDto) => {
    sortTouched.current = true;
    setSortState(mode);
  };

  return { expandedDirs, setExpandedDirs, sortMode, setSortMode };
}
