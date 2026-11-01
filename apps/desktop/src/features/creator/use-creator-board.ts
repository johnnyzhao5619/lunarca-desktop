/**
 * 创作空间的数据（ADR-162）：看板（项目与阶段）由 Rust `creator_board` 给出；拖动卡片后
 * 以 Rust 返回的新看板重画——不在前端乐观地挪卡片，文件才是事实源。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, type CreatorBoardDto, type CreatorBoardViewDto, type ProjectStageDto } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

export interface CreatorBoardState {
  board: CreatorBoardDto | null;
  loading: boolean;
  /** 最近一次读取失败的说明（已经过渲染口 `vaultErrorText`，ADR-105）；`null` = 没失败。 */
  error: string | null;
  reload: () => void;
  /** 把项目移到另一列；失败时返回给人读的说明（已过渲染口），成功返回 `null`。 */
  moveProject: (path: string, stage: ProjectStageDto) => Promise<string | null>;
  /** 显示 / 收起一列（至少一列由 Rust 归一化保证）；失败时返回说明，成功返回 `null`。 */
  setStageVisible: (stage: ProjectStageDto, visible: boolean) => Promise<string | null>;
  /** 切换视图（看板 / 列表 / 表格 / 日历），选择记在设置里；失败时返回说明。 */
  setView: (view: CreatorBoardViewDto) => Promise<string | null>;
  /** 改计划发布日期（`null` = 清掉排期，PRD-19-040）；失败时返回说明。 */
  setPublishDate: (path: string, date: string | null) => Promise<string | null>;
}

/** 看板列配置与视图的设置键：设置页改了它们，看板要跟着重画。 */
const BOARD_SETTING_KEYS: ReadonlySet<string> = new Set(["creator.board_stages", "creator.board_view"]);

/**
 * @param enabled 只在用户站在创作空间、且库已就绪时才读（读看板要逐篇读项目主页）。
 * @param revision 库有变动就重读（`vaultRevision`）：外部编辑器改了阶段、删了项目都要跟上。
 */
export function useCreatorBoard(enabled: boolean, revision: number): CreatorBoardState {
  const [board, setBoard] = useState<CreatorBoardDto | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  /** 只认最后一次请求的回答：快速连续的库变动不能让旧看板盖过新看板。 */
  const latest = useRef(0);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    const request = ++latest.current;
    setLoading(true);
    void commands.creatorBoard().then((result) => {
      if (request !== latest.current) {
        return;
      }
      setLoading(false);
      if (result.status === "ok") {
        setBoard(result.data);
        setError(null);
      } else {
        setError(vaultErrorText(result.error));
      }
    });
  }, [enabled, revision, nonce]);

  const reload = useCallback(() => setNonce((value) => value + 1), []);

  useEffect(() => {
    if (!enabled) {
      return;
    }
    const unlisten = events.settingsChanged.listen((event) => {
      if (BOARD_SETTING_KEYS.has(event.payload.key)) {
        reload();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [enabled, reload]);

  const moveProject = useCallback(async (path: string, stage: ProjectStageDto) => {
    const request = ++latest.current;
    const result = await commands.creatorSetProjectStage(path, stage);
    if (result.status !== "ok") {
      return vaultErrorText(result.error);
    }
    if (request === latest.current) {
      setBoard(result.data);
      setError(null);
    }
    return null;
  }, []);

  const setStageVisible = useCallback(async (stage: ProjectStageDto, visible: boolean) => {
    const request = ++latest.current;
    const result = await commands.creatorSetStageVisible(stage, visible);
    if (result.status !== "ok") {
      return vaultErrorText(result.error);
    }
    if (request === latest.current) {
      setBoard(result.data);
      setError(null);
    }
    return null;
  }, []);

  const setView = useCallback(async (view: CreatorBoardViewDto) => {
    const request = ++latest.current;
    const result = await commands.creatorSetBoardView(view);
    if (result.status !== "ok") {
      return vaultErrorText(result.error);
    }
    if (request === latest.current) {
      setBoard(result.data);
      setError(null);
    }
    return null;
  }, []);

  const setPublishDate = useCallback(async (path: string, date: string | null) => {
    const request = ++latest.current;
    const result = await commands.creatorSetPublishDate(path, date);
    if (result.status !== "ok") {
      return vaultErrorText(result.error);
    }
    if (request === latest.current) {
      setBoard(result.data);
      setError(null);
    }
    return null;
  }, []);

  return { board, loading, error, reload, moveProject, setStageVisible, setView, setPublishDate };
}
