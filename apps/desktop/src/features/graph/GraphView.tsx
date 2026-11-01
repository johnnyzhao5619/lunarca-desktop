// 关系图谱视图主区（PRD-06-034，Obsidian Graph View 对标）：Canvas 力导向渲染 +
// 缩放/平移/拖拽节点/悬停高亮/点击打开；搜索/过滤器/聚焦/AI 标签重扫的控制面归主侧栏
// （PRD-07-057，`GraphSidebar`），状态经 `GraphController` 单一实例共享。数据全部
// 来自 Rust `vault_graph`（ADR-003），颜色/字体经设计 Token（`--graph-*`）从容器
// 计算样式读取，深浅主题自适应。

import {
  Component,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { useTranslation } from "react-i18next";
import { commands, type VaultGraphDto } from "../../ipc/bindings";
import {
  ArrowClockwiseIcon,
  CloseIcon,
  FocusIcon,
  GraphIcon,
  PlusIcon,
  TagsIcon,
} from "../../components/ui/Icon";
import { EmptyState } from "../../components/ui/EmptyState";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator } from "../../lib/context-menu";
import { useNoteMenu } from "../workspace/note-menu";
import {
  ALPHA_DECAY,
  ALPHA_FLOOR,
  buildAdjacency,
  DEFAULT_SIM_PARAMS,
  diagnoseViewport,
  fitTransformFor,
  hitTest,
  initialPosition,
  nodeRadius,
  settleSynchronously,
  SETTLE_ENERGY,
  simulationStep,
  toSimNodes,
  type SimNode,
  type ViewportDiagnosis,
} from "./graph-sim";
import { GraphRelationList } from "./GraphRelationList";
import { GraphRelationHelp } from "./GraphRelationHelp";
import { TopicPreviewCard } from "../knowledge/TopicPreviewCard";
import { goTo } from "../../lib/navigate";
import { memoryFits, recallGraphView, rememberGraphView } from "./graph-view-memory";
import {
  ambientLabelDegreeThreshold,
  fitGraphLabel,
  placeGraphLabels,
  type GraphLabelCandidate,
  type GraphLabelObstacle,
} from "./graph-label-layout";
import type { GraphController } from "./use-graph-controller";

interface GraphViewProps {
  controller: GraphController;
  /** 局部聚焦深链（graph://<path> 导航；nonce 支持重复导航同一笔记）。 */
  focus: { path: string; nonce: number } | null;
  onOpenNote: (path: string) => void;
  onOpenTag: (tag: string) => void;
  /** 状态回流（右键动作的成败反馈）。 */
  onStatus?: (message: string) => void;
  /**
   * 副库窗口的受限模式（PRD-06-050）：菜单去掉 AI 重扫与工作区笔记动作——
   * 它们以主库为落点，副库语境下是跨库操作面；幽灵创建保留（vaultCreate 已窗口感知）。
   */
  restricted?: boolean;
}

/**
 * 画布菜单不高亮归属元素：节点画在 canvas 里没有 DOM 可高亮，而高亮 canvas 本身
 * 等于给整张图铺一层底色（PRD-07-059）。
 */
const NO_HIGHLIGHT = { highlight: null } as const;

/** 缩放边界与交互常量（集中定义，防魔数散落）。 */
const SCALE_MIN = 0.2;
const SCALE_MAX = 4;
/** 指针位移低于此屏幕像素视为点击（否则为拖拽）。 */
const CLICK_SLOP_PX = 4;
const LABEL_PRIORITY = {
  ambient: 0,
  neighbor: 2_000,
  localFocus: 3_000,
  active: 4_000,
} as const;
interface Transform {
  x: number;
  y: number;
  scale: number;
}

/**
 * 从容器计算样式读取图谱 Token（每帧读取开销可忽略；主题切换即时生效）。
 *
 * **字体也要从这里读**：节点标签此前写死成 `11px ui-sans-serif, system-ui, sans-serif`
 * ——字号不在阶梯上（也就不随可访问性字号缩放），字体栈还与 body 的那份不是同一个。
 * 画布上画出来的字与界面上其余的字属于同一套排版，取值口径就该是同一个。
 */
function readGraphStyle(el: HTMLElement) {
  const style = getComputedStyle(el);
  const token = (name: string) => style.getPropertyValue(name).trim();
  return {
    labelFontSize: Number.parseFloat(token("--font-size-xs")) || 12,
    labelFontFamily: style.fontFamily,
    labelMaxWidth: Number.parseFloat(token("--graph-label-max-width")) || 220,
    labelGap: Number.parseFloat(token("--graph-label-gap")) || 6,
    note: token("--graph-node-note"),
    quickNote: token("--graph-node-quick"),
    recording: token("--graph-node-recording"),
    transcript: token("--graph-node-transcript"),
    chat: token("--graph-node-chat"),
    document: token("--graph-node-document"),
    tag: token("--graph-node-tag"),
    topic: token("--graph-node-topic"),
    ghost: token("--graph-node-ghost"),
    edge: token("--graph-edge"),
    label: token("--graph-label"),
    halo: token("--graph-halo"),
    focusRing: token("--graph-focus-ring"),
  };
}

type GraphColors = ReturnType<typeof readGraphStyle>;

function nodeColor(node: SimNode, colors: GraphColors): string {
  if (node.kind === "topic") {
    return colors.topic;
  }
  if (node.kind === "tag") {
    return colors.tag;
  }
  if (node.kind === "ghost") {
    return colors.ghost;
  }
  switch (node.noteType) {
    case "recording":
      return colors.recording;
    case "transcript":
      return colors.transcript;
    case "chat":
      return colors.chat;
    case "document":
      return colors.document;
    case "quick-note":
      return colors.quickNote;
    default:
      return colors.note;
  }
}

/**
 * 打开一页主题（PRD-16-062：双击或回车）。主库落到主题阅读视图；副库窗口没有知识库分区，
 * 按笔记打开（同 `restricted` 下其余动作的取舍）。画布与兜底列表共用这一条。
 */
function openTopicPage(path: string, restricted: boolean, onOpenNote: (path: string) => void) {
  if (restricted) onOpenNote(path);
  else goTo(`topics://${path}`);
}

/**
 * 知识网络（带渲染异常兜底，K7）：画布视图在 React 渲染期抛错时（动画帧与事件回调里的异常不经过这里），不让整块视图变空——
 * 退回同一份过滤后数据的关系列表，节点照样找得到、打得开，并给「重试」回到画布。
 */
export function GraphView(props: GraphViewProps) {
  return (
    <GraphErrorBoundary
      fallback={(retry) => <GraphFallbackList {...props} onRetry={retry} />}
    >
      <GraphCanvasView {...props} />
    </GraphErrorBoundary>
  );
}

class GraphErrorBoundary extends Component<
  { fallback: (retry: () => void) => ReactNode; children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: unknown) {
    // 只记类型，不记内容（日志脱敏）。
    console.error("[graph] 画布视图渲染异常", error instanceof Error ? error.name : typeof error);
  }

  render() {
    return this.state.failed
      ? this.props.fallback(() => this.setState({ failed: false }))
      : this.props.children;
  }
}

function GraphFallbackList({
  controller,
  onOpenNote,
  onOpenTag,
  onRetry,
  restricted = false,
}: GraphViewProps & { onRetry: () => void }) {
  const { t } = useTranslation("workspace");
  return (
    <section className="luna-graph" aria-label={t("graph.title")}>
      <GraphRelationHelp mode={controller.mode} />
      <div className="luna-graph__canvas-wrap">
        <div className="luna-graph__list-panel">
          <p className="luna-graph__list-notice" role="alert">
            {t("graph.list.renderFailed")}{" "}
            <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={onRetry}>
              {t("graph.list.retry")}
            </button>
          </p>
          <GraphRelationList
            nodes={controller.filtered.nodes}
            edges={controller.filtered.edges}
            onOpen={(node) => {
              if (node.kind === "tag") onOpenTag(node.id.slice("tag:".length));
              else if (node.kind === "note") onOpenNote(node.id);
              else if (node.kind === "topic") openTopicPage(node.id, restricted, onOpenNote);
            }}
          />
        </div>
      </div>
    </section>
  );
}

function GraphCanvasView({
  controller,
  focus,
  onOpenNote,
  onOpenTag,
  onStatus,
  restricted = false,
}: GraphViewProps) {
  const { t } = useTranslation("workspace");
  const { t: tCommon } = useTranslation("common");
  const contextMenu = useContextMenu();
  const noteMenu = useNoteMenu();
  /** 上次离开时的视角（只在挂载后第一份数据上用一次）。 */
  const recalledRef = useRef(recallGraphView());
  /** 画布 / 关系列表（同一份数据的两种呈现，PRD-16-052）；回来时沿用上次的选择。 */
  const [view, setView] = useState<"canvas" | "list">(
    () => recalledRef.current?.view ?? "canvas",
  );
  const viewRef = useRef(view);
  viewRef.current = view;
  /** 画布上下文拿不到：只能用关系列表。 */
  const [canvasFailed, setCanvasFailed] = useState(false);
  /**
   * 有节点却一个都不在画布里（K7「空白」）：给「显示全部节点」与只含数字的诊断，而不是
   * 自动挪视角——自动挪会把取证现场抹掉。只在布局静止后判，铺开途中节点飞出界不算。
   */
  const [lostView, setLostView] = useState(false);
  const lostViewRef = useRef(false);
  const diagnosisRef = useRef<{
    diagnosis: ViewportDiagnosis;
    canvas: { cssWidth: number; cssHeight: number; backingWidth: number; backingHeight: number; dpr: number };
  } | null>(null);
  const [diagnosticCopied, setDiagnosticCopied] = useState(false);
  const showList = view === "list" || canvasFailed;
  const {
    data,
    error,
    filtered,
    reload,
    showTags,
    toggleTags,
    showGhosts,
    toggleGhosts,
    showOrphans,
    toggleOrphans,
    focus: focusState,
    setFocus: setFocusState,
    relayoutNonce,
    openRescanDialog,
    mode,
    preview,
    setPreview,
  } = controller;
  const openTopic = useCallback(
    (path: string) => openTopicPage(path, restricted, onOpenNote),
    [onOpenNote, restricted],
  );

  const containerRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  useEffect(() => {
    if (canvasRef.current) canvasRef.current.inert = showList;
  }, [showList]);
  const simNodesRef = useRef<SimNode[]>([]);
  const edgesRef = useRef<VaultGraphDto["edges"]>([]);
  const adjacencyRef = useRef<Map<string, Set<string>>>(new Map());
  const transformRef = useRef<Transform>({ x: 0, y: 0, scale: 1 });
  /** 待执行的一次视口适配（布局静止后兑现；用户一动视角即作废）。 */
  const pendingFitRef = useRef(false);
  /** 本视图是否已经自动适配过——自动适配只在首次拿到数据时发生一次。 */
  const hasFittedRef = useRef(false);
  const alphaRef = useRef(1);
  const dirtyRef = useRef(true);
  const hoverRef = useRef<string | null>(null);
  const keyboardFocusRef = useRef<string | null>(null);
  const rafRef = useRef(0);
  const dragRef = useRef<
    | {
        mode: "node";
        node: SimNode;
        startX: number;
        startY: number;
        moved: boolean;
        openOnClick: boolean;
        withModifier: boolean;
      }
    | { mode: "pan"; startX: number; startY: number; originX: number; originY: number }
    | null
  >(null);

  // 深链聚焦（graph://<path>）：进入局部模式，默认 1 跳。
  useEffect(() => {
    if (focus?.path) {
      setFocusState({ id: focus.path, depth: 1 });
    }
  }, [focus, setFocusState]);

  // 过滤结果变化 → 增量重建模拟节点（保留旧位置不跳图）并重新加热。
  useEffect(() => {
    const recalled = recalledRef.current;
    if (recalled !== null && filtered.nodes.length > 0) {
      recalledRef.current = null;
      const ids = filtered.nodes.map((node) => node.id);
      if (memoryFits(recalled, ids)) {
        // 从笔记回来：接着上次的位置与视角，不重铺、不抢视角；库里多出的节点才让布局再动。
        simNodesRef.current = toSimNodes(filtered.nodes, recalled.positions);
        edgesRef.current = filtered.edges;
        adjacencyRef.current = buildAdjacency(filtered.edges);
        transformRef.current = { ...recalled.transform };
        alphaRef.current = ids.every((id) => recalled.positions.has(id)) ? recalled.alpha : 1;
        hasFittedRef.current = true;
        pendingFitRef.current = recalled.fitPending;
        dirtyRef.current = true;
        return;
      }
    }
    const previous = new Map(simNodesRef.current.map((node) => [node.id, node]));
    simNodesRef.current = toSimNodes(filtered.nodes, previous);
    edgesRef.current = filtered.edges;
    adjacencyRef.current = buildAdjacency(filtered.edges);
    alphaRef.current = 1;
    dirtyRef.current = true;
    // **只有第一次进入才自动适配视口**：之后每次过滤器变化都把视角抢回去，等于
    // 用户每勾一次开关就丢一次自己刚调好的位置。首屏那一次是帮他摆好（画布此前恒以
    // scale:1 起步，十几节点的库渲染出来是正中一小团、四周大片空白）；之后是他的地盘。
    if (!hasFittedRef.current) {
      hasFittedRef.current = true;
      pendingFitRef.current = true;
    }
  }, [filtered]);

  // 离开时记下视角，回来接着看（切到笔记再回来，画布会卸载重挂）。
  useEffect(
    () => () => {
      if (simNodesRef.current.length === 0) return;
      rememberGraphView({
        positions: new Map(simNodesRef.current.map((node) => [node.id, node])),
        transform: { ...transformRef.current },
        alpha: alphaRef.current,
        fitPending: pendingFitRef.current,
        view: viewRef.current,
      });
    },
    [],
  );

  /** 重新布局：抹掉既有位置按初始螺旋重铺（布局纠缠时的一键解药）。 */
  const relayout = useCallback(() => {
    simNodesRef.current = simNodesRef.current.map((node, index) => ({
      ...node,
      ...initialPosition(node.id, index),
      vx: 0,
      vy: 0,
    }));
    alphaRef.current = 1;
    dirtyRef.current = true;
    // 「重新布局」的语义就是「把它理顺、让我看见全貌」，故这一次也适配视口——与首屏
    // 那次同一条通路（等静止后兑现、用户一动即作废），不是第二套机制。
    pendingFitRef.current = true;
  }, []);

  /** 「显示全部节点」：坐标坏了就重新布局，否则只把视角套回全图。 */
  const showAllNodes = useCallback(() => {
    const canvas = canvasRef.current;
    if ((diagnosisRef.current?.diagnosis.nonFinite ?? 0) > 0 || !canvas) {
      relayout();
      return;
    }
    const dpr = window.devicePixelRatio || 1;
    const fit = fitTransformFor(simNodesRef.current, canvas.width / dpr, canvas.height / dpr, SCALE_MIN);
    if (fit) {
      transformRef.current = fit;
      pendingFitRef.current = false;
      dirtyRef.current = true;
    } else {
      relayout();
    }
  }, [relayout]);

  /** 复制诊断：只有数字（画布尺寸、平移缩放、包围盒、计数、主题名），没有任何标题或路径。 */
  const copyDiagnostic = useCallback(() => {
    const snapshot = diagnosisRef.current;
    const root = document.documentElement.dataset;
    const text = JSON.stringify({
      kind: "lunarca-graph-diagnostic",
      canvas: snapshot?.canvas ?? null,
      transform: transformRef.current,
      nodes: snapshot?.diagnosis ?? null,
      alpha: alphaRef.current,
      theme: root.theme ?? null,
      reduceMotion: root.reduceMotion ?? null,
      window: { width: window.innerWidth, height: window.innerHeight },
    });
    void (async () => {
      try {
        await navigator.clipboard.writeText(text);
        setDiagnosticCopied(true);
      } catch {
        setDiagnosticCopied(false);
      }
    })();
  }, []);

  // 侧栏「重新布局」经控制器 nonce 抵达（模拟状态在画布本地）；以 ref 记录已消费值，
  // 视图重挂载时不把初始 nonce 误当成一次点击。
  const consumedRelayoutRef = useRef(relayoutNonce);
  useEffect(() => {
    if (relayoutNonce !== consumedRelayoutRef.current) {
      consumedRelayoutRef.current = relayoutNonce;
      relayout();
    }
  }, [relayoutNonce, relayout]);

  // ── 渲染主循环 ────────────────────────────────────────────────────────────
  useEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    if (!canvas || !container) {
      return;
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) {
      // 画不了（上下文拿不到）：交给同一份数据的关系列表，节点照样找得到、打得开。
      setCanvasFailed(true);
      return;
    }

    const resize = () => {
      const dpr = window.devicePixelRatio || 1;
      const rect = container.getBoundingClientRect();
      canvas.width = Math.max(1, Math.round(rect.width * dpr));
      canvas.height = Math.max(1, Math.round(rect.height * dpr));
      dirtyRef.current = true;
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(container);

    const draw = () => {
      const dpr = window.devicePixelRatio || 1;
      const width = canvas.width / dpr;
      const height = canvas.height / dpr;
      const { x: panX, y: panY, scale } = transformRef.current;
      const colors = readGraphStyle(container);
      const nodes = simNodesRef.current;
      const edges = edgesRef.current;
      const byId = new Map(nodes.map((node) => [node.id, node]));
      const activeNode = hoverRef.current ?? keyboardFocusRef.current;
      const neighbors = activeNode
        ? (adjacencyRef.current.get(activeNode) ?? new Set<string>())
        : null;

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, width, height);
      ctx.translate(width / 2 + panX, height / 2 + panY);
      ctx.scale(scale, scale);

      // 边。主题级网络按关联强度画粗细与浓淡（PRD-16-062：几乎每页都引用的枢纽来源只算一点，
      // 否则图是一张分不出远近的全连通网）；笔记级网络的强度恒为 1，外观不变。
      for (const edge of edges) {
        const a = byId.get(edge.source);
        const b = byId.get(edge.target);
        if (!a || !b) {
          continue;
        }
        const connected =
          activeNode !== null && (edge.source === activeNode || edge.target === activeNode);
        const weighted = edge.kind === "source";
        const strength = edge.strength / 100;
        ctx.lineWidth = (weighted ? 0.5 + 2.5 * strength : 1) / scale;
        ctx.globalAlpha = activeNode
          ? connected
            ? 0.85
            : 0.06
          : weighted
            ? 0.12 + 0.48 * strength
            : 0.3;
        ctx.strokeStyle = colors.edge;
        ctx.setLineDash(edge.kind === "tag" ? [3 / scale, 3 / scale] : []);
        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        ctx.lineTo(b.x, b.y);
        ctx.stroke();
      }
      ctx.setLineDash([]);

      // 节点。
      for (const node of nodes) {
        const radius = nodeRadius(node);
        const isHover = node.id === activeNode;
        const isNeighbor = neighbors?.has(node.id) ?? false;
        const dimmed = activeNode !== null && !isHover && !isNeighbor;
        ctx.globalAlpha = node.kind === "ghost" ? (dimmed ? 0.12 : 0.45) : dimmed ? 0.18 : 1;
        if (isHover) {
          // 光晕：halo Token 为实色（canvas 不解析 color-mix），半透明由 globalAlpha 实现。
          ctx.globalAlpha = 0.18;
          ctx.beginPath();
          ctx.arc(node.x, node.y, radius + 6 / scale, 0, Math.PI * 2);
          ctx.fillStyle = colors.halo;
          ctx.fill();
          ctx.globalAlpha = 1;
        }
        ctx.beginPath();
        ctx.arc(node.x, node.y, radius, 0, Math.PI * 2);
        ctx.fillStyle = nodeColor(node, colors);
        ctx.fill();
        if (focusState && node.id === focusState.id) {
          ctx.globalAlpha = 1;
          ctx.strokeStyle = colors.focusRing;
          ctx.lineWidth = 2 / scale;
          ctx.beginPath();
          ctx.arc(node.x, node.y, radius + 3 / scale, 0, Math.PI * 2);
          ctx.stroke();
          ctx.lineWidth = 1 / scale;
        }
      }

      // 标签采用屏幕空间 LOD + 碰撞避让：总览只留高连接度节点，放大后逐层显露；
      // 悬停 / 键盘焦点及邻域优先。文字在屏幕坐标绘制，缩放时字号保持可读。
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.fillStyle = colors.label;
      ctx.textAlign = "left";
      ctx.textBaseline = "top";
      ctx.font = `${colors.labelFontSize}px ${colors.labelFontFamily}`;
      const labelHeight = colors.labelFontSize * 1.25;
      const degreeThreshold = ambientLabelDegreeThreshold(scale, nodes.length);
      const candidates: GraphLabelCandidate[] = [];
      // 全部节点圆点都是标签的静态障碍物——**包括没有标签的那些**（PRD-06-034）。
      // 只避让「有标签的节点」是不够的：被压住的那个点恰恰多半是没标签的低连接度节点，
      // 而它被盖住之后用户连"这里有个东西"都看不出来。
      const obstacles: GraphLabelObstacle[] = nodes.map((node) => ({
        id: node.id,
        centerX: width / 2 + panX + node.x * scale,
        centerY: height / 2 + panY + node.y * scale,
        radius: nodeRadius(node) * scale,
      }));
      for (const node of nodes) {
        const isHover = node.id === activeNode;
        const isNeighbor = neighbors?.has(node.id) ?? false;
        const isFocus = focusState?.id === node.id;
        const ambient =
          activeNode === null &&
          node.degree >= degreeThreshold &&
          node.kind !== "ghost";
        if (!isHover && !isNeighbor && !isFocus && !ambient) {
          continue;
        }
        const text = fitGraphLabel(
          node.label,
          colors.labelMaxWidth,
          (value) => ctx.measureText(value).width,
        );
        const labelWidth = ctx.measureText(text).width;
        const screenX = width / 2 + panX + node.x * scale;
        const screenY = height / 2 + panY + node.y * scale;
        const radius = nodeRadius(node) * scale;
        // 节点本身在画布外就不给标签：标签布局会把它夹回可视区，节点全跑出视野时
        // 标签会沿边缘排成一列、像是没有节点的文字（真机 2026-09-27）。
        if (
          screenX < -radius ||
          screenX > width + radius ||
          screenY < -radius ||
          screenY > height + radius
        ) {
          continue;
        }
        // 首选放节点下方（阅读顺序），备选翻到上方——密集处下方多半正压着另一个圆点，
        // 没有备选位就等于「碰上即丢」，标签会成片消失（真机 15 节点从 7 个掉到 3 个）。
        const below = screenY + radius + colors.labelGap;
        const above = screenY - radius - colors.labelGap - labelHeight;
        const fitsBelow = below + labelHeight <= height - colors.labelGap;
        candidates.push({
          id: node.id,
          text,
          centerX: screenX,
          top: fitsBelow ? below : above,
          altTop: fitsBelow ? above : below,
          width: labelWidth,
          height: labelHeight,
          priority: isHover
            ? LABEL_PRIORITY.active
            : isFocus
              ? LABEL_PRIORITY.localFocus
              : isNeighbor
                ? LABEL_PRIORITY.neighbor + node.degree
                : LABEL_PRIORITY.ambient + node.degree,
          required: isHover || isFocus,
        });
      }
      const placements = placeGraphLabels(
        candidates,
        { width, height, inset: colors.labelGap, collisionGap: colors.labelGap },
        obstacles,
      );
      for (const placement of placements) {
        const isActive = placement.id === activeNode;
        const isNeighbor = neighbors?.has(placement.id) ?? false;
        ctx.globalAlpha = isActive ? 1 : isNeighbor ? 0.9 : 0.72;
        ctx.fillText(placement.text, placement.left, placement.top);
      }
      ctx.globalAlpha = 1;

      const diagnosis = diagnoseViewport(nodes, transformRef.current, width, height);
      diagnosisRef.current = {
        diagnosis,
        canvas: {
          cssWidth: width,
          cssHeight: height,
          backingWidth: canvas.width,
          backingHeight: canvas.height,
          dpr,
        },
      };
      const lost =
        nodes.length > 0 && alphaRef.current === 0 && width > 2 && height > 2 && diagnosis.visible === 0;
      if (lost !== lostViewRef.current) {
        lostViewRef.current = lost;
        setLostView(lost);
      }
    };

    const loop = () => {
      if (alphaRef.current > ALPHA_FLOOR) {
        if (document.documentElement.dataset.reduceMotion === "true") {
          // 减少动态效果：同一套模拟一次算到静止，直接呈现终态，不播布局动画。
          alphaRef.current = settleSynchronously(
            simNodesRef.current,
            edgesRef.current,
            DEFAULT_SIM_PARAMS,
            alphaRef.current,
          );
        } else {
          const energy = simulationStep(
            simNodesRef.current,
            edgesRef.current,
            DEFAULT_SIM_PARAMS,
            alphaRef.current,
          );
          alphaRef.current *= ALPHA_DECAY;
          if (energy < SETTLE_ENERGY && alphaRef.current < 0.4) {
            alphaRef.current = 0; // 已静止：停跑模拟（交互会重新加热）。
          }
        }
        dirtyRef.current = true;
      }
      // 布局静止后把整张图放进视口，**每份数据只做一次**（PRD-06-034 总览优先）。
      // 等静止再适配：布局还在铺开时包围盒每帧都在变，边算边套等于让整张图漂移。
      // 用户一旦自己平移/缩放/拖节点，`pendingFitRef` 当场清零——适配是"我还没
      // 表态时帮我摆好"，不是"每次都把我的视角抢回去"。
      // 画布还没有真实尺寸（视图被隐藏时为 1×1）就不适配：对着零视口算出的缩放毫无意义，
      // 留到画布有了尺寸再做。
      const hasViewport = canvas.width > 2 && canvas.height > 2;
      if (pendingFitRef.current && alphaRef.current === 0 && hasViewport) {
        pendingFitRef.current = false;
        const dpr = window.devicePixelRatio || 1;
        const fit = fitTransformFor(
          simNodesRef.current,
          canvas.width / dpr,
          canvas.height / dpr,
          SCALE_MIN,
        );
        if (fit) {
          transformRef.current = fit;
          dirtyRef.current = true;
        }
      }
      if (dirtyRef.current) {
        dirtyRef.current = false;
        draw();
      }
      rafRef.current = requestAnimationFrame(loop);
    };
    rafRef.current = requestAnimationFrame(loop);

    return () => {
      cancelAnimationFrame(rafRef.current);
      observer.disconnect();
    };
  }, [focusState]);

  // ── 指针交互 ──────────────────────────────────────────────────────────────
  const toWorld = useCallback((clientX: number, clientY: number) => {
    const canvas = canvasRef.current;
    const rect = canvas?.getBoundingClientRect();
    const { x, y, scale } = transformRef.current;
    const width = rect?.width ?? 0;
    const height = rect?.height ?? 0;
    return {
      x: (clientX - (rect?.left ?? 0) - width / 2 - x) / scale,
      y: (clientY - (rect?.top ?? 0) - height / 2 - y) / scale,
    };
  }, []);

  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLCanvasElement>) => {
      if (event.button !== 0) {
        return;
      }
      event.currentTarget.setPointerCapture(event.pointerId);
      const world = toWorld(event.clientX, event.clientY);
      const scale = transformRef.current.scale;
      const node = hitTest(simNodesRef.current, world.x, world.y, 6 / scale);
      if (node) {
        node.pinned = true;
        dragRef.current = {
          mode: "node",
          node,
          startX: event.clientX,
          startY: event.clientY,
          moved: false,
          openOnClick: node.kind !== "ghost",
          withModifier: event.metaKey || event.ctrlKey,
        };
      } else {
        const { x, y } = transformRef.current;
        dragRef.current = {
          mode: "pan",
          startX: event.clientX,
          startY: event.clientY,
          originX: x,
          originY: y,
        };
        event.currentTarget.style.cursor = "grabbing";
      }
    },
    [toWorld],
  );

  const onPointerMove = useCallback(
    (event: ReactPointerEvent<HTMLCanvasElement>) => {
      const drag = dragRef.current;
      const canvas = event.currentTarget;
      if (!drag) {
        // 悬停命中测试（变化才标脏重绘）。
        const world = toWorld(event.clientX, event.clientY);
        const scale = transformRef.current.scale;
        const node = hitTest(simNodesRef.current, world.x, world.y, 6 / scale);
        const nextHover = node?.id ?? null;
        if (nextHover !== hoverRef.current) {
          hoverRef.current = nextHover;
          canvas.style.cursor = node && node.kind !== "ghost" ? "pointer" : "default";
          dirtyRef.current = true;
        }
        return;
      }
      const dx = event.clientX - drag.startX;
      const dy = event.clientY - drag.startY;
      // 用户开始摆布这张图了：那次还没兑现的自动适配就此作废，别把他的视角抢回去。
      pendingFitRef.current = false;
      if (drag.mode === "pan") {
        transformRef.current.x = drag.originX + dx;
        transformRef.current.y = drag.originY + dy;
        dirtyRef.current = true;
        return;
      }
      if (Math.hypot(dx, dy) > CLICK_SLOP_PX) {
        drag.moved = true;
      }
      const world = toWorld(event.clientX, event.clientY);
      drag.node.x = world.x;
      drag.node.y = world.y;
      alphaRef.current = Math.max(alphaRef.current, 0.3);
      dirtyRef.current = true;
    },
    [toWorld],
  );

  /** 中断收尾（pointercancel / 捕获丢失）：只解除拖拽状态，绝不触发点击打开。 */
  const onPointerCancel = useCallback((event: ReactPointerEvent<HTMLCanvasElement>) => {
    const drag = dragRef.current;
    dragRef.current = null;
    event.currentTarget.style.cursor = "default";
    if (drag?.mode === "node") {
      drag.node.pinned = false;
    }
  }, []);

  const activateNode = useCallback(
    (node: Pick<SimNode, "id" | "kind">) => {
      // 主题节点单击 = 预览（ADR-136 ④）；笔记节点仍单击即打开（2026-09-27 裁决不变）。
      if (node.kind === "topic") {
        setPreview(node.id);
      } else if (node.kind === "tag") {
        onOpenTag(node.id.slice("tag:".length));
      } else if (node.kind === "note") {
        onOpenNote(node.id);
      } else {
        setFocusState({ id: node.id, depth: focusState?.depth ?? 1 });
      }
    },
    [focusState?.depth, onOpenNote, onOpenTag, setFocusState, setPreview],
  );

  /** 双击主题节点即打开；其余节点的双击没有额外含义（单击已经打开）。 */
  const onDoubleClick = useCallback(
    (event: ReactMouseEvent<HTMLCanvasElement>) => {
      const world = toWorld(event.clientX, event.clientY);
      const node = hitTest(simNodesRef.current, world.x, world.y, 6 / transformRef.current.scale);
      if (node?.kind === "topic") {
        openTopic(node.id);
      }
    },
    [openTopic, toWorld],
  );

  const onPointerUp = useCallback(
    (event: ReactPointerEvent<HTMLCanvasElement>) => {
      const drag = dragRef.current;
      dragRef.current = null;
      event.currentTarget.style.cursor = "default";
      if (!drag || drag.mode === "pan") {
        return;
      }
      drag.node.pinned = false;
      if (drag.moved) {
        return;
      }
      const { node } = drag;
      if (drag.withModifier) {
        // ⌘/Ctrl+点击：以该节点为中心进入局部聚焦（Obsidian 局部图谱心智）。
        setFocusState({ id: node.id, depth: focusState?.depth ?? 1 });
        return;
      }
      if (!drag.openOnClick) {
        return;
      }
      activateNode(node);
    },
    [activateNode, focusState, setFocusState],
  );

  const clearPointerHover = useCallback(() => {
    if (dragRef.current) return;
    if (hoverRef.current !== null) {
      hoverRef.current = null;
      dirtyRef.current = true;
    }
  }, []);

  /**
   * 画布右键（PRD-07-059）：命中节点给该节点的动作，落空给整张图的动作。
   *
   * Canvas 里没有 DOM 节点可挂事件，故命中测试与左键同一口径（`hitTest` + 同一
   * 世界坐标换算），不另写一份判定。
   */
  const onCanvasContextMenu = useCallback(
    (event: ReactMouseEvent<HTMLCanvasElement>) => {
      const world = toWorld(event.clientX, event.clientY);
      const scale = transformRef.current.scale;
      const node = hitTest(simNodesRef.current, world.x, world.y, 6 / scale);
      if (!node) {
        contextMenu.open(
          event,
          buildMenu([
            {
              kind: "action",
              id: "relayout",
              label: t("graph.relayout"),
              icon: <ArrowClockwiseIcon size={13} />,
              onSelect: relayout,
            },
            {
              kind: "action",
              id: "refresh",
              label: tCommon("menu.refresh"),
              icon: <ArrowClockwiseIcon size={13} />,
              onSelect: reload,
            },
            restricted ? null : separator("ai"),
            restricted
              ? null
              : {
                  kind: "action" as const,
                  id: "aiRescan",
                  label: t("graph.rescan"),
                  icon: <TagsIcon size={13} />,
                  onSelect: () => openRescanDialog(null),
                },
            separator("filters"),
            {
              kind: "action",
              id: "showTags",
              label: t("graph.showTags"),
              checked: showTags,
              onSelect: toggleTags,
            },
            {
              kind: "action",
              id: "showGhosts",
              label: t("graph.showGhosts"),
              checked: showGhosts,
              onSelect: toggleGhosts,
            },
            {
              kind: "action",
              id: "showOrphans",
              label: t("graph.showOrphans"),
              checked: showOrphans,
              onSelect: toggleOrphans,
            },
            focusState ? separator("focus") : null,
            focusState && {
              kind: "action",
              id: "exitFocus",
              label: t("graph.exitFocus"),
              icon: <CloseIcon size={13} />,
              onSelect: () => setFocusState(null),
            },
          ]),
          NO_HIGHLIGHT,
        );
        return;
      }

      const focusItems = ([1, 2] as const).map((depth) => ({
        kind: "action" as const,
        id: `focus-${depth}`,
        label: t("graph.focusDepth", { depth }),
        icon: <FocusIcon size={13} />,
        checked: focusState?.id === node.id && focusState.depth === depth,
        onSelect: () => setFocusState({ id: node.id, depth }),
      }));

      if (node.kind === "topic") {
        contextMenu.open(
          event,
          buildMenu([
            {
              kind: "action",
              id: "openTopic",
              label: t("graph.openTopic"),
              onSelect: () => openTopic(node.id),
            },
            {
              kind: "action",
              id: "previewTopic",
              label: t("graph.previewTopic"),
              onSelect: () => setPreview(node.id),
            },
            separator("focus"),
            ...focusItems,
          ]),
          NO_HIGHLIGHT,
        );
        return;
      }

      if (node.kind === "tag") {
        const tag = node.id.slice("tag:".length);
        contextMenu.open(
          event,
          buildMenu([
            {
              kind: "action",
              id: "openTag",
              label: t("tags.filterByTag"),
              icon: <TagsIcon size={13} />,
              onSelect: () => onOpenTag(tag),
            },
            separator("focus"),
            ...focusItems,
          ]),
          NO_HIGHLIGHT,
        );
        return;
      }

      if (node.kind === "ghost") {
        // 幽灵节点 = 被链接但尚不存在的笔记。右键**就地把它变成真的**——
        // 这是图谱里唯一能兑现"空心圈是待写的东西"的动作。
        contextMenu.open(
          event,
          buildMenu([
            {
              kind: "action",
              id: "createGhost",
              label: t("graph.createNote"),
              icon: <PlusIcon size={13} />,
              onSelect: () => {
                void commands.vaultCreate(`${node.label}.md`, "note", "").then((result) => {
                  if (result.status === "error") {
                    onStatus?.(result.error);
                    return;
                  }
                  reload();
                  onOpenNote(result.data.path);
                });
              },
            },
            separator("focus"),
            ...focusItems,
          ]),
          NO_HIGHLIGHT,
        );
        return;
      }

      if (restricted) {
        // 工作区笔记菜单（分屏/独立窗/蒸馏/AI 标签/导出…）全部以主库为落点，
        // 副库图谱只给「打开 + 聚焦」。
        contextMenu.open(
          event,
          buildMenu([
            {
              kind: "action",
              id: "open",
              label: t("graph.openNote"),
              onSelect: () => onOpenNote(node.id),
            },
            separator("focus"),
            ...focusItems,
          ]),
          NO_HIGHLIGHT,
        );
        return;
      }
      contextMenu.open(
        event,
        noteMenu({
          path: node.id,
          openInNotes: () => onOpenNote(node.id),
          leading: focusItems,
          onStatus: onStatus ?? (() => {}),
          onChanged: reload,
        }),
        NO_HIGHLIGHT,
      );
    },
    [
      contextMenu,
      focusState,
      setFocusState,
      reload,
      noteMenu,
      onOpenNote,
      onOpenTag,
      onStatus,
      openRescanDialog,
      openTopic,
      setPreview,
      relayout,
      showGhosts,
      showOrphans,
      showTags,
      toggleTags,
      toggleGhosts,
      toggleOrphans,
      restricted,
      t,
      tCommon,
      toWorld,
    ],
  );

  // 滚轮缩放走原生非被动监听（React onWheel 为被动，preventDefault 无效——缩放时
  // 会连带滚动外层容器）；以光标为锚：光标下的世界点保持不动。
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) {
      return;
    }
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      // 同 onPointerMove：用户自己缩放之后，那次待兑现的自动适配作废。
      pendingFitRef.current = false;
      const rect = canvas.getBoundingClientRect();
      const transform = transformRef.current;
      const factor = Math.exp(-event.deltaY * 0.0015);
      const next = Math.min(SCALE_MAX, Math.max(SCALE_MIN, transform.scale * factor));
      const cx = event.clientX - rect.left - rect.width / 2;
      const cy = event.clientY - rect.top - rect.height / 2;
      transform.x = cx - ((cx - transform.x) / transform.scale) * next;
      transform.y = cy - ((cy - transform.y) / transform.scale) * next;
      transform.scale = next;
      dirtyRef.current = true;
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      canvas.removeEventListener("wheel", onWheel);
    };
  }, []);

  const isEmpty = data !== null && filtered.nodes.length === 0;

  return (
    <section className="luna-graph" aria-label={t("graph.title")}>
      <GraphRelationHelp mode={mode} />
      <div ref={containerRef} className="luna-graph__canvas-wrap">
        <canvas
          ref={canvasRef}
          className="luna-graph__canvas"
          aria-hidden={showList}
          aria-label={t("graph.canvasSummary", {
            nodes: filtered.nodes.length,
            edges: filtered.edges.length,
          })}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerCancel}
          onPointerLeave={clearPointerHover}
          onContextMenu={onCanvasContextMenu}
          onDoubleClick={onDoubleClick}
        >
          <ul>
            {filtered.nodes.map((node) => {
              const topic = node.kind === "topic";
              const labelKey = topic
                ? "graph.accessibleTopicNode"
                : node.kind === "tag"
                  ? "graph.accessibleTagNode"
                  : node.kind === "ghost"
                    ? "graph.accessibleGhostNode"
                    : "graph.accessibleNoteNode";
              return (
                <li key={node.id}>
                  <button
                    type="button"
                    tabIndex={0}
                    onFocus={() => {
                      keyboardFocusRef.current = node.id;
                      dirtyRef.current = true;
                      // 键盘：聚焦主题节点即预览，回车打开（ADR-136 ④）。
                      if (topic) setPreview(node.id);
                    }}
                    onBlur={() => {
                      if (keyboardFocusRef.current === node.id) {
                        keyboardFocusRef.current = null;
                        dirtyRef.current = true;
                      }
                    }}
                    onClick={() => (topic ? openTopic(node.id) : activateNode(node))}
                  >
                    {t(labelKey, { label: node.label, count: node.degree })}
                  </button>
                </li>
              );
            })}
          </ul>
        </canvas>
        {/* 三态各答各的（此前是两行共用同一个裸 `<p>`）：
            ① 出错 —— 是错误不是空态，故 `role="alert"`，与全应用错误行一致；
            ② 筛空 —— 库里有节点、是过滤器把它们藏了，给一条把过滤器清干净的出路
              （搜索与标签的「无匹配」态一直是这么做的，图谱此前只说"没有匹配"）；
            ③ 真空 —— 库里就是没有可画的东西，只说明状况。 */}
        {error ? (
          <p className="luna-graph__overlay luna-graph__error" role="alert">
            {error}{" "}
            {/* 读失败给一条出路（PRD-16-052「加载失败有出口」），不只是一句报错。 */}
            <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={reload}>
              {t("common:action.retry")}
            </button>
          </p>
        ) : data === null ? (
          // 第一次读取中：说「加载中」，不让空画布 + 0 节点看起来像图谱空了（真机 2026-09-27，
          // 大一点的库要读几秒）。
          <div className="luna-graph__overlay" role="status">
            <p className="luna-empty">{tCommon("status.loading")}</p>
          </div>
        ) : isEmpty ? (
          <div className="luna-graph__overlay">
            {data && data.nodes.length > 0 ? (
              <EmptyState
                icon={<GraphIcon size={24} />}
                title={t("graph.noMatch")}
                action={
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    onClick={controller.resetFilters}
                  >
                    {t("graph.resetFilters")}
                  </button>
                }
              />
            ) : (
              <EmptyState
                icon={<GraphIcon size={24} />}
                title={t(mode === "topics" ? "graph.topicEmpty" : "graph.empty")}
              />
            )}
          </div>
        ) : lostView && !showList ? (
          <div className="luna-graph__overlay">
            <EmptyState
              icon={<GraphIcon size={24} />}
              title={t("graph.lost.title")}
              action={
                <>
                  <button type="button" className="luna-btn luna-btn--primary" onClick={showAllNodes}>
                    {t("graph.lost.showAll")}
                  </button>
                  <button type="button" className="luna-btn luna-btn--secondary" onClick={copyDiagnostic}>
                    {t(diagnosticCopied ? "graph.lost.copied" : "graph.lost.copy")}
                  </button>
                </>
              }
            />
          </div>
        ) : null}
        {showList && !isEmpty && !error && (
          <div className="luna-graph__list-panel">
            {canvasFailed && (
              <p className="luna-graph__list-notice" role="status">
                {t("graph.list.canvasFailed")}
              </p>
            )}
            <GraphRelationList nodes={filtered.nodes} edges={filtered.edges} onOpen={activateNode} />
          </div>
        )}
        {/* 主题预览卡（ADR-136 ④）：只在主题网络里、且那一页仍在这张图上时出现。 */}
        {mode === "topics" && preview !== null && filtered.nodes.some((node) => node.id === preview) && (
          <TopicPreviewCard path={preview} onOpen={openTopic} onClose={() => setPreview(null)} />
        )}
        {!isEmpty && !error && (
          <div className="luna-graph__view-switch" role="group" aria-label={t("graph.list.switch")}>
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              aria-pressed={!showList}
              disabled={canvasFailed}
              onClick={() => setView("canvas")}
            >
              {t("graph.list.canvas")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              aria-pressed={showList}
              onClick={() => setView("list")}
            >
              {t("graph.list.list")}
            </button>
          </div>
        )}
      </div>
      {!showList && <p className="luna-graph__hint">{t(mode === "topics" ? "graph.topicHint" : "graph.hint")}</p>}
    </section>
  );
}
