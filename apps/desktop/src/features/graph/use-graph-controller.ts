// 关系图谱控制器（PRD-06-034/035）：侧栏（搜索 / 过滤器 / AI 关系标签重扫）与主区画布
// 共享单一状态实例（PRD-07-057——侧栏承载控制、主区呈现内容，与搜索 / 设置同构）。
//
// 前端零业务（ADR-003）：图数据与重扫判定全在 Rust，过滤是 `graph-sim` 纯函数；本 Hook
// 只管交互状态、请求节奏与事件订阅。`active` 门控数据加载——图谱数据只在目的地激活时
// 拉取与跟随 `vault://changed` 重载（控制器常驻 WorkspaceShell，不能让未打开的图谱
// 每次笔记保存都全库构图）；重扫进度订阅则常驻（无查询开销，且重扫跨目的地继续跑）。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  commands,
  events,
  type GraphRescanPreflightDto,
  type GraphRescanProgress,
  type VaultGraphDto,
} from "../../ipc/bindings";
import { GRAPH_RESCAN_EVENT, type GraphRescanRequestDetail } from "../../lib/app-events";
import {
  DEFAULT_GRAPH_FILTER,
  DEFAULT_VISIBLE_NOTE_TYPES,
  filterGraph,
  type GraphFilter,
} from "./graph-sim";
import { vaultErrorText } from "../../lib/vault-error";
import { useVaultReload } from "../../hooks/use-vault-reload";

/** `vault://changed` 触发重载的防抖（批量写盘只重建一次图）。 */

/**
 * 网络的粒度（PRD-16-062 / ADR-136 ④）：`topics` = 主题页为节点、共享来源为边（Rust
 * `topic_network`）；`notes` = 笔记、标签与未创建链接（`vault_graph`）。默认主题级；库里还没有
 * 主题页时自动落到笔记级（那时主题网络是一片空白），用户自己选过之后不再替他换。
 */
export type GraphMode = "topics" | "notes";

export interface GraphFocusState {
  id: string;
  depth: number;
}

/** 重扫确认卡状态：关闭 / 预检中 / 就绪（带盘点数字）/ 预检失败。
 * `scope` 贯穿全程（PRD-06-035c 定向重扫）：`null` = 整个库。 */
/**
 * 确认卡里哪一档是主动作。
 *
 * 从「标签语言分裂」那张维护卡进来时必须是 `force`（PRD-06-035d ②）：语言不一致的
 * 笔记**指纹是最新的**，「只补缺口」按指纹跳过，一篇都扫不到。默认的主次在那个场景下
 * 恰好是反的——把用户引向一颗按了什么都不发生（或干脆是禁用）的按钮。
 */
export type RescanEmphasis = "gaps" | "force";

export type RescanDialogState =
  | { kind: "closed" }
  | { kind: "loading"; scope: string | null; emphasis: RescanEmphasis }
  | {
      kind: "ready";
      scope: string | null;
      emphasis: RescanEmphasis;
      preflight: GraphRescanPreflightDto;
      startError: string | null;
    }
  | { kind: "error"; scope: string | null; message: string };

export interface GraphController {
  mode: GraphMode;
  setMode: (mode: GraphMode) => void;
  /** 主题网络里单击选中、正在预览的那一页（路径）；`null` = 没有预览卡。 */
  preview: string | null;
  setPreview: (path: string | null) => void;
  data: VaultGraphDto | null;
  error: string | null;
  reload: () => void;
  filtered: { nodes: VaultGraphDto["nodes"]; edges: VaultGraphDto["edges"] };
  query: string;
  setQuery: (value: string) => void;
  showTags: boolean;
  toggleTags: () => void;
  showGhosts: boolean;
  toggleGhosts: () => void;
  showOrphans: boolean;
  toggleOrphans: () => void;
  /** 可见笔记类型（ADR-024）：资料区默认不在其中。 */
  noteTypes: readonly string[];
  toggleNoteType: (type: string) => void;
  /**
   * 显示记录态笔记（ADR-086）：默认关。**与类型正交**——它是成熟度维度，不是第六种类型。
   */
  showDrafts: boolean;
  toggleDrafts: () => void;
  /**
   * 把「看不到东西」的全部成因一次清干净：搜索词 + 四组过滤器 + 聚焦。
   *
   * 筛空空态必须给得出这条出路——否则用户只知道"没有匹配"，却要自己回想是哪一个开关
   * 把节点藏了（搜索与标签面板的无匹配态一直有「清空」按钮，图谱此前只有一句话）。
   */
  resetFilters: () => void;
  focus: GraphFocusState | null;
  setFocus: (focus: GraphFocusState | null) => void;
  focusLabel: string | null;
  /** 重新布局请求（侧栏按钮 → 画布模拟；自增 nonce，画布 effect 消费）。 */
  relayoutNonce: number;
  requestRelayout: () => void;
  /** AI 关系标签重扫（PRD-06-035b/035c）：`scope = null` 整库，否则定向文件夹。 */
  rescanDialog: RescanDialogState;
  openRescanDialog: (scope?: string | null, emphasis?: RescanEmphasis) => void;
  closeRescanDialog: () => void;
  /** 进行中的重扫进度；`null` = 无重扫在跑（终态帧即清空，完成态由通知承载）。 */
  rescanProgress: GraphRescanProgress | null;
  /** 启动重扫；`scope` 由确认卡显式传入（即其打开时的范围）。 */
  startRescan: (force: boolean, scope: string | null) => Promise<void>;
  cancelRescan: () => void;
}

export function useGraphController(active: boolean): GraphController {
  const [mode, setModeState] = useState<GraphMode>("topics");
  /** 用户亲手选过粒度：之后即使主题网络为空也不替他换回笔记级。 */
  const modeChosenRef = useRef(false);
  const [preview, setPreview] = useState<string | null>(null);
  /** 自增请求序号：切换粒度后，上一种网络迟到的结果不落地。 */
  const requestRef = useRef(0);
  const [data, setData] = useState<VaultGraphDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [showTags, setShowTags] = useState(true);
  const [showGhosts, setShowGhosts] = useState(true);
  const [showOrphans, setShowOrphans] = useState(true);
  const [noteTypes, setNoteTypes] = useState<readonly string[]>(DEFAULT_VISIBLE_NOTE_TYPES);
  /** 记录态默认隐藏、可开（PRD-06-052）：与资料同为可逆视图态，构图与索引不变。 */
  const [showDrafts, setShowDrafts] = useState(DEFAULT_GRAPH_FILTER.showDrafts);
  const [focus, setFocus] = useState<GraphFocusState | null>(null);
  const [relayoutNonce, setRelayoutNonce] = useState(0);
  const [rescanDialog, setRescanDialog] = useState<RescanDialogState>({ kind: "closed" });
  const [rescanProgress, setRescanProgress] = useState<GraphRescanProgress | null>(null);

  const load = useCallback(async () => {
    const request = ++requestRef.current;
    const result = mode === "topics" ? await commands.topicNetwork() : await commands.vaultGraph();
    if (request !== requestRef.current) {
      return;
    }
    if (result.status === "ok" && mode === "topics" && result.data.nodes.length === 0 && !modeChosenRef.current) {
      setModeState("notes");
      return;
    }
    if (result.status === "ok") {
      setData(result.data);
      setError(null);
    } else {
      setError(vaultErrorText(result.error));
    }
  }, [mode]);

  const setMode = useCallback(
    (next: GraphMode) => {
      modeChosenRef.current = true;
      if (next === mode) return;
      // 换了一种网络：旧数据、聚焦与预览都属于另一张图。
      setData(null);
      setFocus(null);
      setPreview(null);
      setModeState(next);
    },
    [mode],
  );

  // 数据加载随目的地激活（笔记增删改 / AI 标签落定都会改变图结构）。
  useEffect(() => {
    if (!active) {
      return;
    }
    void load();
  }, [active, load]);
  // 库变了就重画（合并同一阵变更；此前这里自己抄了一份去抖）。
  useVaultReload(() => void load(), { enabled: active });

  // 重扫进度常驻订阅：终态帧清空进行中状态（完成/中止由通知中心告知，避免两处收尾文案）。
  useEffect(() => {
    // 事件到过就以事件为准：挂载时的快照查询可能在终态帧之后才回来，若仍应用会把
    // 已收尾的进度条复活成"永远差一格"。
    let sawEvent = false;
    const unlisten = events.graphRescan.listen((event) => {
      sawEvent = true;
      setRescanProgress(event.payload.finished ? null : event.payload);
    });
    // 挂载时恢复快照：界面重载期间重扫仍在后台跑，不能让进度显示凭空消失。
    void commands.graphRescanStatus().then((result) => {
      if (!sawEvent && result.status === "ok" && result.data !== null) {
        setRescanProgress(result.data);
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const filter: GraphFilter = useMemo(
    () => ({ query, showTags, showGhosts, showOrphans, noteTypes, showDrafts, focus }),
    [query, showTags, showGhosts, showOrphans, noteTypes, showDrafts, focus],
  );

  const filtered = useMemo(
    () => (data ? filterGraph(data.nodes, data.edges, filter) : { nodes: [], edges: [] }),
    [data, filter],
  );

  const focusLabel = useMemo(() => {
    if (!focus || !data) {
      return null;
    }
    return data.nodes.find((node) => node.id === focus.id)?.label ?? focus.id;
  }, [focus, data]);

  const openRescanDialog = useCallback(
    (scope: string | null = null, emphasis: RescanEmphasis = "gaps") => {
      setRescanDialog({ kind: "loading", scope, emphasis });
      void commands.graphRescanPreflight(scope).then((result) => {
        if (result.status === "ok") {
          setRescanDialog({
            kind: "ready",
            scope,
            emphasis,
            preflight: result.data,
            startError: null,
          });
        } else {
          setRescanDialog({ kind: "error", scope, message: result.error });
        }
      });
    },
    [],
  );

  const closeRescanDialog = useCallback(() => {
    setRescanDialog({ kind: "closed" });
  }, []);

  // 定向重扫请求的唯一宿主（文件树右键派发，PRD-06-035c）：确认卡在 WorkspaceShell
  // 层常驻，故任何目的地都能唤起，不必先切到图谱。
  useEffect(() => {
    const onRequest = (event: Event) => {
      const detail = (event as CustomEvent<GraphRescanRequestDetail>).detail;
      openRescanDialog(detail.scope, detail.emphasis ?? "gaps");
    };
    window.addEventListener(GRAPH_RESCAN_EVENT, onRequest);
    return () => window.removeEventListener(GRAPH_RESCAN_EVENT, onRequest);
  }, [openRescanDialog]);

  const startRescan = useCallback(async (force: boolean, scope: string | null) => {
    const result = await commands.graphRescanStart(force, scope);
    if (result.status === "ok") {
      setRescanDialog({ kind: "closed" });
      return;
    }
    setRescanDialog((current) =>
      current.kind === "ready" ? { ...current, startError: vaultErrorText(result.error) } : current,
    );
  }, []);

  const cancelRescan = useCallback(() => {
    void commands.graphRescanCancel();
  }, []);

  return {
    mode,
    setMode,
    preview,
    setPreview,
    data,
    error,
    reload: () => void load(),
    filtered,
    query,
    setQuery,
    showTags,
    toggleTags: () => setShowTags((value) => !value),
    showGhosts,
    toggleGhosts: () => setShowGhosts((value) => !value),
    showOrphans,
    toggleOrphans: () => setShowOrphans((value) => !value),
    noteTypes,
    showDrafts,
    toggleDrafts: () => setShowDrafts((value) => !value),
    toggleNoteType: (type: string) =>
      setNoteTypes((current) =>
        current.includes(type) ? current.filter((t) => t !== type) : [...current, type],
      ),
    resetFilters: () => {
      setQuery("");
      setShowTags(true);
      setShowGhosts(true);
      setShowOrphans(true);
      setNoteTypes(DEFAULT_VISIBLE_NOTE_TYPES);
      setShowDrafts(DEFAULT_GRAPH_FILTER.showDrafts);
      setFocus(null);
    },
    focus,
    setFocus,
    focusLabel,
    relayoutNonce,
    requestRelayout: () => setRelayoutNonce((value) => value + 1),
    rescanDialog,
    openRescanDialog,
    closeRescanDialog,
    rescanProgress,
    startRescan,
    cancelRescan,
  };
}
