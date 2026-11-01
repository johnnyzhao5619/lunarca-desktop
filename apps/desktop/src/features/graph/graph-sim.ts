// 关系图谱纯逻辑（PRD-06-034，Obsidian Graph View 对标）：过滤 / 邻域 / 力导向布局。
// 零 DOM 依赖，可无头测试；渲染与交互归 GraphView（业务数据与语义全部来自 Rust
// `vault_graph`，本模块只做展示层几何/集合运算，ADR-003）。

import type { GraphEdgeDto, GraphNodeDto } from "../../ipc/bindings";

/** 参与布局的节点（在 DTO 上叠加位置/速度；id 等语义字段只读透传）。 */
export interface SimNode {
  readonly id: string;
  readonly label: string;
  readonly kind: string;
  readonly noteType: string | null;
  readonly degree: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
  /** 拖拽中固定（力不作用；松手恢复）。 */
  pinned: boolean;
}

/** 力导向参数（单一来源；数值经真机手感调优，无魔数散落）。 */
export interface SimParams {
  /** 多体斥力强度（库仑式，随距离平方衰减）。 */
  repulsion: number;
  /** 边弹簧目标长度（px，世界坐标）。 */
  linkDistance: number;
  /** 边弹簧强度（0–1）。 */
  linkStrength: number;
  /** 向心引力强度（把游离簇拉回视野中心）。 */
  centerStrength: number;
  /** 速度阻尼（每 tick 保留比例）。 */
  damping: number;
  /** 斥力单次作用上限（防近距爆炸）。 */
  maxRepulsion: number;
}

export const DEFAULT_SIM_PARAMS: SimParams = {
  repulsion: 1800,
  linkDistance: 110,
  linkStrength: 0.06,
  centerStrength: 0.012,
  damping: 0.82,
  maxRepulsion: 12,
};

/** 确定性字符串哈希（FNV-1a 32 位）：初始布局与抖动的种子，无随机数。 */
export function hashId(id: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < id.length; i += 1) {
    hash ^= id.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/**
 * 初始布局：黄金角螺旋按序铺开（半径 ∝ √序号，密度均匀），叠加 id 哈希微抖动
 * （同一图重复构建位置一致——确定性利于测试与视觉稳定）。
 */
export function initialPosition(id: string, index: number): { x: number; y: number } {
  const GOLDEN_ANGLE = 2.399963229728653;
  const radius = 24 * Math.sqrt(index + 1);
  const angle = index * GOLDEN_ANGLE + (hashId(id) % 997) / 997;
  return { x: radius * Math.cos(angle), y: radius * Math.sin(angle) };
}

/** DTO → Sim 节点（`previous` 中同 id 节点保留既有位置，重载不跳图）。 */
export function toSimNodes(
  nodes: readonly GraphNodeDto[],
  previous: ReadonlyMap<string, SimNode>,
): SimNode[] {
  return nodes.map((node, index) => {
    const prior = previous.get(node.id);
    const position = prior ?? initialPosition(node.id, index);
    return {
      id: node.id,
      label: node.label,
      kind: node.kind,
      noteType: node.note_type,
      degree: node.degree,
      x: position.x,
      y: position.y,
      vx: prior?.vx ?? 0,
      vy: prior?.vy ?? 0,
      pinned: prior?.pinned ?? false,
    };
  });
}

/** 邻接表（无向）。 */
export function buildAdjacency(
  edges: readonly GraphEdgeDto[],
): Map<string, Set<string>> {
  const adjacency = new Map<string, Set<string>>();
  const link = (a: string, b: string) => {
    let set = adjacency.get(a);
    if (!set) {
      set = new Set();
      adjacency.set(a, set);
    }
    set.add(b);
  };
  for (const edge of edges) {
    link(edge.source, edge.target);
    link(edge.target, edge.source);
  }
  return adjacency;
}

/** 以 `focus` 为中心 BFS `depth` 跳的节点集（含中心；中心不在图中返回仅中心）。 */
export function neighborhood(
  adjacency: ReadonlyMap<string, ReadonlySet<string>>,
  focus: string,
  depth: number,
): Set<string> {
  const seen = new Set<string>([focus]);
  let frontier = [focus];
  for (let hop = 0; hop < depth; hop += 1) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const neighbor of adjacency.get(id) ?? []) {
        if (!seen.has(neighbor)) {
          seen.add(neighbor);
          next.push(neighbor);
        }
      }
    }
    frontier = next;
  }
  return seen;
}

/**
 * 图谱里可显示的笔记类型（与 Rust `NoteType::key()` kebab 键同源）。
 * 顺序即侧栏类型过滤器的呈现顺序。
 */
export const GRAPH_NOTE_TYPES = [
  "note",
  "quick-note",
  "recording",
  "transcript",
  "chat",
  "document",
  "video-project",
  "script",
  "swipe",
] as const;

export type GraphNoteType = (typeof GRAPH_NOTE_TYPES)[number];

/**
 * 默认可见的笔记类型（ADR-024 资料/知识分层）：只有**知识**默认入图——
 * 普通笔记与速记。资料区（录音 / 转写 / 对话）默认隐藏，一键可见。
 *
 * 这是**视图态**不是数据态：构图与索引一字未改，资料照常可搜（`type:` 精确检索）、
 * 照常在笔记树里；隐藏的只是"知识的形状"这一屏上的噪音。
 *
 * 创作空间的三类（项目主页 / 脚本 / 灵感，ADR-162）都是用户写的正文，按知识默认入图。
 */
export const DEFAULT_VISIBLE_NOTE_TYPES: readonly GraphNoteType[] = [
  "note",
  "quick-note",
  "video-project",
  "script",
  "swipe",
];

/** 视图过滤器（Obsidian Graph 过滤区对标）。 */
export interface GraphFilter {
  /** 搜索词（命中节点 + 其直接邻居保留；空 = 不过滤）。 */
  query: string;
  /** 显示标签节点。 */
  showTags: boolean;
  /** 显示未创建笔记（幽灵节点）。 */
  showGhosts: boolean;
  /** 显示孤立节点（无可见边）。 */
  showOrphans: boolean;
  /** 可见的笔记类型（ADR-024；缺 `note_type` 的节点按 `note` 计——与索引缺省口径一致）。 */
  noteTypes: readonly string[];
  /**
   * 显示**记录态**笔记（ADR-086 成熟度轴）。默认 `false` = 默认隐藏、可开。
   *
   * **与 `noteTypes` 正交，不是第六种类型**：一篇记录可以是 `note` 也可以是速记，
   * 它是同一批节点的另一个维度。两个条件按**与**生效——类型被关掉的记录不会因为
   * 打开这个开关就回来。与资料同为可逆视图态：构图与索引一字未改。
   */
  showDrafts: boolean;
  /** 局部聚焦：以某节点为中心的 N 跳邻域（null = 全局图）。 */
  focus: { id: string; depth: number } | null;
}

export const DEFAULT_GRAPH_FILTER: GraphFilter = {
  query: "",
  showTags: true,
  showGhosts: true,
  showOrphans: true,
  noteTypes: DEFAULT_VISIBLE_NOTE_TYPES,
  showDrafts: false,
  focus: null,
};

/** 过滤结果（节点 + 两端均可见的边）。 */
export interface FilteredGraph {
  nodes: GraphNodeDto[];
  edges: GraphEdgeDto[];
}

/**
 * 应用过滤器。顺序有意为之：
 * 1. 类别开关（标签/幽灵）与笔记类型先裁剪节点池；
 * 2. 局部聚焦在裁剪后的边上 BFS（关掉标签节点后聚焦不会经标签"跳"到无关笔记）；
 * 3. 搜索词保留命中节点及其直接邻居（有上下文可读）；
 * 4. 孤立过滤最后按"可见边"判定（前序过滤可能制造新孤立点）。
 *
 * 第 4 步对**笔记类型**与既有三个开关一视同仁（隐藏资料后只连着资料的笔记会成为新孤立
 * 点，关掉「显示孤立节点」时随之隐去）——「孤立」的语义是"在当前这张图里没有连接"，
 * 按可见边判定才自洽，也与 Obsidian 一致；为类型单开一条"按全图度数判定"的特例，
 * 会让同一个开关对不同过滤源给出两套行为。
 */
export function filterGraph(
  nodes: readonly GraphNodeDto[],
  edges: readonly GraphEdgeDto[],
  filter: GraphFilter,
): FilteredGraph {
  const visibleTypes = new Set(filter.noteTypes);
  let kept = nodes.filter(
    (node) =>
      (filter.showTags || node.kind !== "tag") &&
      (filter.showGhosts || node.kind !== "ghost") &&
      // 缺省 `note_type` 按 `note` 计，与派生索引「无 luna.type 一律视为 note」同口径。
      (node.kind !== "note" || visibleTypes.has(node.note_type ?? "note")) &&
      // 成熟度轴（ADR-086）与类型轴**并列**裁剪：`draft` 是 Rust 已判定的布尔，
      // 标签/幽灵节点恒 `false` 故不受影响。
      (filter.showDrafts || !node.draft),
  );
  let keptIds = new Set(kept.map((node) => node.id));
  let visibleEdges = edges.filter(
    (edge) => keptIds.has(edge.source) && keptIds.has(edge.target),
  );

  if (filter.focus) {
    const zone = neighborhood(
      buildAdjacency(visibleEdges),
      filter.focus.id,
      Math.max(1, filter.focus.depth),
    );
    kept = kept.filter((node) => zone.has(node.id));
    keptIds = new Set(kept.map((node) => node.id));
    visibleEdges = visibleEdges.filter(
      (edge) => keptIds.has(edge.source) && keptIds.has(edge.target),
    );
  }

  const query = filter.query.trim().toLowerCase();
  if (query) {
    const adjacency = buildAdjacency(visibleEdges);
    const matches = new Set(
      kept
        .filter((node) => node.label.toLowerCase().includes(query))
        .map((node) => node.id),
    );
    const withNeighbors = new Set(matches);
    for (const id of matches) {
      for (const neighbor of adjacency.get(id) ?? []) {
        withNeighbors.add(neighbor);
      }
    }
    kept = kept.filter((node) => withNeighbors.has(node.id));
    keptIds = new Set(kept.map((node) => node.id));
    visibleEdges = visibleEdges.filter(
      (edge) => keptIds.has(edge.source) && keptIds.has(edge.target),
    );
  }

  if (!filter.showOrphans) {
    const connected = new Set<string>();
    for (const edge of visibleEdges) {
      connected.add(edge.source);
      connected.add(edge.target);
    }
    // 聚焦中心即使孤立也保留（用户显式选中的锚点不该消失）。
    kept = kept.filter(
      (node) => connected.has(node.id) || node.id === filter.focus?.id,
    );
  }

  return { nodes: kept, edges: visibleEdges };
}

/**
 * 力导向一步（就地更新位置/速度）：多体斥力 + 边弹簧 + 向心引力，`alpha`（0–1）
 * 统一缩放力度（随时间衰减 → 收敛静止）。返回本步总动能（调用方据此停帧省电）。
 */
export function simulationStep(
  nodes: SimNode[],
  edges: readonly GraphEdgeDto[],
  params: SimParams,
  alpha: number,
): number {
  const byId = new Map(nodes.map((node) => [node.id, node]));

  // 多体斥力（对称作用；O(n²) 带近距上限，MVP 库规模足够）。
  for (let i = 0; i < nodes.length; i += 1) {
    const a = nodes[i];
    for (let j = i + 1; j < nodes.length; j += 1) {
      const b = nodes[j];
      let dx = b.x - a.x;
      let dy = b.y - a.y;
      let distSq = dx * dx + dy * dy;
      if (distSq === 0) {
        // 完全重叠：按哈希确定性偏移，避免 NaN 且保持无随机。
        dx = ((hashId(a.id) % 7) - 3) / 10 || 0.1;
        dy = ((hashId(b.id) % 7) - 3) / 10 || -0.1;
        distSq = dx * dx + dy * dy;
      }
      const dist = Math.sqrt(distSq);
      const force = Math.min(params.maxRepulsion, params.repulsion / distSq) * alpha;
      const fx = (dx / dist) * force;
      const fy = (dy / dist) * force;
      a.vx -= fx;
      a.vy -= fy;
      b.vx += fx;
      b.vy += fy;
    }
  }

  // 边弹簧（Hooke 向目标长度收敛；标签边略短——标签是聚类中心，Obsidian 观感）。
  for (const edge of edges) {
    const a = byId.get(edge.source);
    const b = byId.get(edge.target);
    if (!a || !b) {
      continue;
    }
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const dist = Math.sqrt(dx * dx + dy * dy) || 0.1;
    // 关联强的两页拉得更近（主题级网络的强度 1–100，笔记级恒为 100、长度与拉力都不变）：
    // 弱关联的目标长度最长到 1.6 倍、拉力最低到 0.15 倍——仍连着，只是不把整张图揉成一团。
    const strength = edge.strength / 100;
    const base = edge.kind === "tag" ? params.linkDistance * 0.8 : params.linkDistance;
    const target = base * (1 + 0.6 * (1 - strength));
    const stretch = (dist - target) / dist;
    const pull = params.linkStrength * Math.max(0.15, strength);
    const fx = dx * stretch * pull * alpha;
    const fy = dy * stretch * pull * alpha;
    a.vx += fx;
    a.vy += fy;
    b.vx -= fx;
    b.vy -= fy;
  }

  // 向心引力 + 积分。
  let kinetic = 0;
  for (const node of nodes) {
    node.vx -= node.x * params.centerStrength * alpha;
    node.vy -= node.y * params.centerStrength * alpha;
    node.vx *= params.damping;
    node.vy *= params.damping;
    if (node.pinned) {
      node.vx = 0;
      node.vy = 0;
      continue;
    }
    node.x += node.vx;
    node.y += node.vy;
    kinetic += node.vx * node.vx + node.vy * node.vy;
  }
  return kinetic;
}

/** 节点渲染半径（连接度映射，√ 压缩长尾；Obsidian 同心智）。 */
export function nodeRadius(node: Pick<SimNode, "degree" | "kind">): number {
  // 主题节点更大一档：主题级网络里每个点都是一整页汇总，不是一篇笔记。
  const base = node.kind === "topic" ? 6 : node.kind === "tag" ? 5 : node.kind === "ghost" ? 3 : 4;
  return base + Math.min(9, Math.sqrt(node.degree) * 1.8);
}

/** 命中测试：世界坐标点选最近节点（含 padding 容差），无命中返回 null。 */
export function hitTest(
  nodes: readonly SimNode[],
  worldX: number,
  worldY: number,
  padding: number,
): SimNode | null {
  let best: SimNode | null = null;
  let bestDistSq = Number.POSITIVE_INFINITY;
  for (const node of nodes) {
    const reach = nodeRadius(node) + padding;
    const dx = node.x - worldX;
    const dy = node.y - worldY;
    const distSq = dx * dx + dy * dy;
    if (distSq <= reach * reach && distSq < bestDistSq) {
      best = node;
      bestDistSq = distSq;
    }
  }
  return best;
}

/** 视口适配的结果：与画布 `transformRef` 同构（世界坐标 → 屏幕）。 */
export interface FitTransform {
  x: number;
  y: number;
  scale: number;
}

/** 适配时四周留出的空白（屏幕 px）——贴着边缘的节点会被标签与光晕裁掉。 */
const FIT_PADDING = 72;

/**
 * 适配的**放大上限**。
 *
 * 一张三节点的图若按比例撑满画布，节点会大得像气泡、边长得像管道——「占满画布」从来
 * 不是目的，「一眼看清全貌」才是。缩小则不设下限之外的限制：图再大也该先让人看到形状。
 *
 * **取值由「放大后节点多大」倒推，不是拍脑袋**：`nodeRadius` 的基数是 4–5 世界单位，
 * 2.5× 之后渲染半径 10–12.5px（直径 20–25px），正是常见图谱工具的节点尺寸。
 *
 * **1.4 那一版太保守，真机上等于没适配**（2026-08-30 实测）：15 节点的库力导后世界
 * 跨度约 207 单位，1.4× 之后在 1545px 宽的画布上只占 290px——上限成了实际生效的约束，
 * 图照旧缩在正中一小团，而适配本身是白跑的。放大不会让标签变糊：标签在**屏幕空间**
 * 以固定字号绘制，放大只会让节点散开、标签更放得下。
 */
const FIT_MAX_SCALE = 2.5;

/**
 * 把整张图放进视口（Obsidian / Gephi 的 zoom-to-fit 同款）。
 *
 * 画布此前恒以 `scale: 1` 起步，于是一个十几节点的库渲染出来是**画布正中一小团**、
 * 四周大片空白——用户既看不清那团里有什么，也没有任何线索说明该滚轮放大。适配一次
 * 之后，同一张图铺开占据视口，标签（屏幕空间绘制、字号恒定）随之有地方放得下。
 *
 * 纯函数、零 DOM：适配算的是数字，不该为了测它去造一个 canvas。
 *
 * @returns `null` = 没有节点或视口尺寸无效，调用方保持当前变换不动。
 */
export function fitTransformFor(
  nodes: readonly Pick<SimNode, "x" | "y">[],
  width: number,
  height: number,
  scaleMin: number,
): FitTransform | null {
  if (nodes.length === 0 || width <= 0 || height <= 0) {
    return null;
  }
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const node of nodes) {
    if (!Number.isFinite(node.x) || !Number.isFinite(node.y)) {
      continue; // 仿真尚未产生有效位置的节点不参与包围盒，免得把它拉成 NaN。
    }
    minX = Math.min(minX, node.x);
    maxX = Math.max(maxX, node.x);
    minY = Math.min(minY, node.y);
    maxY = Math.max(maxY, node.y);
  }
  if (!Number.isFinite(minX) || !Number.isFinite(minY)) {
    return null;
  }
  const centerX = (minX + maxX) / 2;
  const centerY = (minY + maxY) / 2;
  const usableWidth = Math.max(1, width - FIT_PADDING * 2);
  const usableHeight = Math.max(1, height - FIT_PADDING * 2);
  // 单节点或一条直线时某一边跨度为 0：那一轴不构成约束，交给另一轴（都为 0 就取上限）。
  const spanX = maxX - minX;
  const spanY = maxY - minY;
  const scaleX = spanX > 0 ? usableWidth / spanX : Infinity;
  const scaleY = spanY > 0 ? usableHeight / spanY : Infinity;
  const raw = Math.min(scaleX, scaleY);
  const scale = Math.min(FIT_MAX_SCALE, Math.max(scaleMin, Number.isFinite(raw) ? raw : FIT_MAX_SCALE));
  // 画布把世界原点画在 (width/2 + panX, height/2 + panY)，故把包围盒中心送到画布中心
  // 只需 pan = -center * scale。
  return { x: -centerX * scale, y: -centerY * scale, scale };
}

/** 每一步的温度衰减与静止阈值——动画循环与一次算完共用，两处不各写一份。 */
export const ALPHA_DECAY = 0.985;
export const ALPHA_FLOOR = 0.02;
export const SETTLE_ENERGY = 0.05;

/**
 * 低动态效果（系统或应用设置「减少动态效果」）：同一套模拟一次算到静止，画面直接呈现终态，
 * 不播放布局动画。步数有上限，超大图也只卡一帧。返回剩余温度（恒为 0：交给调用方停跑）。
 */
export function settleSynchronously(
  nodes: SimNode[],
  edges: readonly GraphEdgeDto[],
  params: SimParams,
  alpha: number,
  maxSteps = 600,
): number {
  let current = alpha;
  for (let step = 0; step < maxSteps && current > ALPHA_FLOOR; step += 1) {
    const energy = simulationStep(nodes, edges, params, current);
    current *= ALPHA_DECAY;
    if (energy < SETTLE_ENERGY && current < 0.4) break;
  }
  return 0;
}

/** 一个节点的一条关系（关系列表用；只转述 DTO 里的边，不推断支持或因果）。 */
export interface NodeRelation {
  id: string;
  label: string;
  kind: "links_to" | "linked_from" | "tag" | "tagged_note" | "shares_source";
}

const RELATION_ORDER: Record<NodeRelation["kind"], number> = {
  links_to: 0,
  linked_from: 1,
  tag: 2,
  tagged_note: 3,
  shares_source: 4,
};

/**
 * 某节点在这批边里的全部关系：笔记的「链接到 / 被链接 / 带的标签」，标签节点的「带这个标签的
 * 笔记」。按关系种类、再按名称排序；同一邻居同一种关系只列一次。
 */
export function relationsOf(
  nodeId: string,
  nodes: readonly GraphNodeDto[],
  edges: readonly GraphEdgeDto[],
): NodeRelation[] {
  const labels = new Map(nodes.map((node) => [node.id, node.label]));
  const seen = new Set<string>();
  const out: NodeRelation[] = [];
  const push = (id: string, kind: NodeRelation["kind"]) => {
    const key = `${kind}\u0000${id}`;
    if (seen.has(key) || !labels.has(id)) return;
    seen.add(key);
    out.push({ id, label: labels.get(id) ?? id, kind });
  };
  for (const edge of edges) {
    if (edge.kind === "source") {
      // 主题级网络（PRD-16-062）：共享来源是对称关系，两端各记一次「共享来源」。
      if (edge.source === nodeId) push(edge.target, "shares_source");
      if (edge.target === nodeId) push(edge.source, "shares_source");
    } else if (edge.kind === "tag") {
      if (edge.source === nodeId) push(edge.target, "tag");
      if (edge.target === nodeId) push(edge.source, "tagged_note");
    } else {
      if (edge.source === nodeId) push(edge.target, "links_to");
      if (edge.target === nodeId) push(edge.source, "linked_from");
    }
  }
  return out.sort(
    (a, b) => RELATION_ORDER[a.kind] - RELATION_ORDER[b.kind] || a.label.localeCompare(b.label),
  );
}

/** 画布此刻的可见性诊断（K7「空白」取证）：只含数字，不含节点标题或路径。 */
export interface ViewportDiagnosis {
  total: number;
  /** 屏幕坐标落在画布内的节点数。 */
  visible: number;
  /** 坐标不是有限数（NaN / Infinity）的节点数：一个非有限值会经力模拟扩散到整张图。 */
  nonFinite: number;
  /** 有限坐标节点的世界坐标包围盒；一个都没有为 `null`。 */
  bbox: { minX: number; maxX: number; minY: number; maxY: number } | null;
}

/**
 * 按与绘制相同的换算（画布中心 + 平移 + 缩放）数一数有多少节点落在画布里。
 * 平移缩放本身不是有限数时，一个节点也不算可见。
 */
export function diagnoseViewport(
  nodes: readonly Pick<SimNode, "x" | "y">[],
  transform: FitTransform,
  width: number,
  height: number,
): ViewportDiagnosis {
  const transformOk =
    Number.isFinite(transform.x) && Number.isFinite(transform.y) && Number.isFinite(transform.scale);
  let visible = 0;
  let nonFinite = 0;
  let bbox: ViewportDiagnosis["bbox"] = null;
  for (const node of nodes) {
    if (!Number.isFinite(node.x) || !Number.isFinite(node.y)) {
      nonFinite += 1;
      continue;
    }
    bbox = bbox
      ? {
          minX: Math.min(bbox.minX, node.x),
          maxX: Math.max(bbox.maxX, node.x),
          minY: Math.min(bbox.minY, node.y),
          maxY: Math.max(bbox.maxY, node.y),
        }
      : { minX: node.x, maxX: node.x, minY: node.y, maxY: node.y };
    if (!transformOk) continue;
    const screenX = width / 2 + transform.x + node.x * transform.scale;
    const screenY = height / 2 + transform.y + node.y * transform.scale;
    if (screenX >= 0 && screenX <= width && screenY >= 0 && screenY <= height) {
      visible += 1;
    }
  }
  return { total: nodes.length, visible, nonFinite, bbox };
}

