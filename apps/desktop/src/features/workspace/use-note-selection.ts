/**
 * 清单/文件树的多选状态与键盘契约（PRD-06-059）——**一处实现**，笔记树与五条清单共用
 * （录音 / 转写 / 标签结果 / 速记 / AI 对话历史）。
 *
 * 选择代数是纯函数（`note-selection.ts`），这里只做三件 React 侧的事：
 * ① 跟随外部「当前对象」（打开的标签、回放中的那一篇）收敛；② 随可见行收敛；
 * ③ 兑现声明出去的 ARIA 角色所承诺的键盘交互。
 *
 * **「当前打开的那一条」与「选中的这一批」是两件事**：前者答"我在看什么"
 * （`aria-current`，工作面跟随它），后者答"接下来的动作作用在谁身上"（`aria-selected`）。
 * 把两者合成一个 `selectedPath` 正是多选做不出来的原因——Cmd 加选一条录音若顺手把
 * 右侧回放也切过去，用户每加选一条就被打断一次。故加选/连选**不触发打开**。
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import {
  orderedSelection,
  prunedSelection,
  rowAtOffset,
  selectionAfterClick,
  selectionForContextMenu,
  singleSelection,
  type SelectionState,
} from "./note-selection";

/** 行按钮上的路径标记：方向键要把焦点搬到下一行，得先在 DOM 里找得到它。
    模块私有——视图从 `rowProps` 拿接线，不该自己往行上贴这个属性。

    它同时是**选中态外观的唯一判据**：`workspace.css` 里那两条
    `[data-row-path][aria-selected="true"]` 就挂在它上面。改名要连着那两条一起改。 */
const ROW_PATH_ATTR = "data-row-path";

/** 承载键盘契约的那个容器（行的兜底聚焦目标）。同上，视图从 `containerProps` 拿。 */
const ROWS_CONTAINER_ATTR = "data-rows";

export interface NoteSelectionOptions {
  /** 当前打开/正在看的那一条（`aria-current` 的对象）；由调用方持有。 */
  current: string | null;
  /** 此刻**可见**的行，按屏幕顺序。选择集与锚点恒在它上面收敛。 */
  visible: readonly string[];
  /** 方向键落到某一行时展开/收起（文件树的 ← →；平铺清单不传即不给这两键）。 */
  onToggleExpand?: (path: string, expand: boolean) => void;
  /**
   * Delete 键删除**选中的这一批**（不传即不给这个键）。
   *
   * **Mac 键盘上标着 delete 的那个键，浏览器里叫 `Backspace`**（`Delete` 是 fn+它）——
   * 只认 `Delete` 的实现在 Mac 上等于没做。确认卡由调用方那条既有删除通道给，故这里
   * 不需要、也不该知道"删掉之前要问什么"。
   */
  onDelete?: () => void;
}

export interface NoteSelection {
  /** 选中路径，按所见顺序——批量命令一律用它提交。 */
  ordered: string[];
  /** 多选中（>1）：批量文案、批量菜单与批量工具栏的唯一判据。 */
  multiple: boolean;
  contains: (path: string) => boolean;
  /**
   * 行左键。返回 `true` = 这一次点击是**打开**（无修饰键的单选），调用方据此发
   * `onSelect`；返回 `false` = 这一次只是在改选择集，不该动工作面。
   */
  click: (path: string, event: SelectionMouseEvent) => boolean;
  /** 右键落点（落在选中集里保持整批，否则单选它）。 */
  contextClick: (path: string) => void;
  /**
   * 右键落在这一行上时，动作真正作用的那一批。
   *
   * **必须与 `contextClick` 同步给出，不能等状态更新后再读 `ordered`**：`contextClick`
   * 排的是一次 React 状态更新，而菜单在**同一拍**里就要装配好条目——右键一行选中集之外
   * 的笔记时，那时读到的 `ordered` 还是上一批，菜单会作用在用户没有右键的对象上。
   */
  targetsFor: (path: string) => string[];
  /**
   * 挂在**承载这些行的容器**上：键盘契约（↑↓ 移动、Shift+↑↓ 连选、Home/End 到两端、
   * ←→ 展开收起、Esc 收回多选、Delete 删除）全在这里兑现。
   */
  containerProps: {
    tabIndex: number;
    onKeyDown: (event: KeyboardEvent<HTMLElement>) => void;
  } & Record<string, unknown>;
  /**
   * 挂在**行**上：路径标记（方向键据它找下一行）、`aria-selected` 与"把焦点留在这一行"。
   *
   * **焦点必须显式抢**：macOS 的 WKWebView（Safari 同行为）点击 `<button>` 不给焦点，
   * 且 mousedown 的**默认行为会把焦点收回 body**——只在 pointerdown 里 `focus()` 会被它
   * 随后一笔勾销。表现是所有按键都落在 body 上：↑↓ 变成滚动页面、空格翻页、Esc 与
   * Delete 毫无反应（2026-09-02 真机）。故 `preventDefault()` 掐掉那个默认行为再聚焦；
   * 连程序性聚焦都被拒时退一步聚焦容器——键盘处理器就挂在那儿，焦点在这棵子树里即可。
   */
  rowProps: (path: string) => {
    onMouseDown: (event: ReactMouseEvent<HTMLElement>) => void;
  } & Record<string, unknown>;
}

/** 键落在文本输入面上（`<input>` / `<textarea>` / contenteditable）。 */
function editableTarget(node: HTMLElement | null): boolean {
  const tag = node?.tagName?.toLowerCase();
  return tag === "input" || tag === "textarea" || node?.isContentEditable === true;
}

/** 只取判定需要的字段：纯逻辑不认识 React 的合成事件。 */
export interface SelectionMouseEvent {
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
}

export function useNoteSelection(options: NoteSelectionOptions): NoteSelection {
  const { current, visible, onToggleExpand, onDelete } = options;
  const [selection, setSelection] = useState<SelectionState>(() => singleSelection(current));
  const currentRef = useRef(current);

  // 外部把当前对象换掉了（点开另一个标签、创建后跳转、删除后清空）→ 选择集跟着回到单选。
  // 只在**它真的变了**时收敛：否则本组件自己的加选会在下一次渲染里被立刻撤销。
  useEffect(() => {
    if (currentRef.current === current) {
      return;
    }
    currentRef.current = current;
    setSelection((state) => (current !== null && state.paths.has(current) ? state : singleSelection(current)));
  }, [current]);

  // 可见行变了（折叠、筛选、外部删除）→ 丢掉已不在场的那些。
  //
  // 剪空之后要**重新落到"当前那条"上**：清单的可见行是子组件**上报**上来的（它自己
  // 的 effect），所以第一帧的可见行必然是空的。照直剪，初始那条单选就在挂载的第一帧被
  // 剪掉了，而它只在 `current` 变化时才重建——于是第一次 Cmd 加选只亮起一行（本该两行），
  // Shift 连选更是连锚点都没有，直接退化成单选（真机 2026-09-09：速记与 AI 对话「无法
  // 多选」）。「当前那条恒在选择集里」本就是这个 Hook 的不变式，这里把它补齐。
  useEffect(() => {
    setSelection((state) => {
      const pruned = prunedSelection(state, visible);
      const current = currentRef.current;
      if (pruned.paths.size === 0 && current !== null && visible.includes(current)) {
        return singleSelection(current);
      }
      return pruned;
    });
  }, [visible]);

  const ordered = useMemo(() => orderedSelection(selection, visible), [selection, visible]);

  /** 把焦点搬到某一行。**这里刻意不 `preventScroll`**：方向键走到屏幕外的那一行时，
      本来就该把它滚进来（与鼠标点击那一处相反——那里滚动会把正要拖的行挪走）。 */
  const focusRow = useCallback((container: HTMLElement | null, path: string) => {
    const row = container?.querySelector<HTMLElement>(`[${ROW_PATH_ATTR}="${CSS.escape(path)}"]`);
    row?.focus();
  }, []);

  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLElement>) => {
      const container = event.currentTarget;
      const source = event.target as HTMLElement;
      // **落在输入面上的按键归输入框**：清单里就有输入框（文件夹改名 / 树内新建），
      // 在那儿按 ↑↓ 是"在这串字里移动光标"，被列表当成"换一行选中"会让人当场失去
      // 正在编辑的那一行；Esc 同理（它该取消这次编辑，而不是收回多选）。
      if (editableTarget(source)) {
        return;
      }
      // 键盘的"此刻站在哪一行"= 选择的**动端**（`lead`），焦点只是它的投影：平台可能
      // 不给按钮焦点，而选择是状态，永远问得出答案。↑↓ / Shift+↑↓ / ←→ 都从它出发。
      const cursor = selection.lead ?? currentRef.current;
      if (event.key === "Escape") {
        setSelection((state) => (state.paths.size > 1 ? singleSelection(currentRef.current) : state));
        return;
      }
      if ((event.key === "Delete" || event.key === "Backspace") && onDelete) {
        event.preventDefault();
        onDelete();
        return;
      }
      if ((event.key === "ArrowRight" || event.key === "ArrowLeft") && onToggleExpand && cursor) {
        event.preventDefault();
        onToggleExpand(cursor, event.key === "ArrowRight");
        return;
      }
      const offset =
        event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
      const jump = event.key === "Home" ? "first" : event.key === "End" ? "last" : null;
      if (offset === 0 && jump === null) {
        return;
      }
      const target =
        jump === null
          ? rowAtOffset(visible, cursor, offset)
          : (jump === "first" ? visible[0] : visible[visible.length - 1]) ?? null;
      if (target === null || target === undefined) {
        return;
      }
      event.preventDefault();
      setSelection((state) =>
        // Shift+方向键 = 从锚点连选到落点；无修饰键 = 只把选择挪过去（**不打开**，
        // 打开归 Enter/空格——按住方向键滑过十行不该顺手加载十篇笔记）。
        event.shiftKey
          ? selectionAfterClick(state, target, { toggle: false, range: true }, visible)
          : singleSelection(target),
      );
      focusRow(container, target);
    },
    [focusRow, onDelete, onToggleExpand, selection.lead, visible],
  );

  return {
    ordered,
    multiple: selection.paths.size > 1,
    contains: (path) => selection.paths.has(path),
    click: (path, event) => {
      const modifiers = { toggle: event.metaKey || event.ctrlKey, range: event.shiftKey };
      setSelection((state) => selectionAfterClick(state, path, modifiers, visible));
      return !modifiers.toggle && !modifiers.range;
    },
    containerProps: {
      // `-1`：容器不进 Tab 序（行本身是按钮，逐行可达的既有键盘故事不变），但**可以被
      // 程序性聚焦**——那是行拒绝接受焦点时的落点。
      tabIndex: -1,
      [ROWS_CONTAINER_ATTR]: "",
      onKeyDown,
    },
    rowProps: (path) => ({
      [ROW_PATH_ATTR]: path,
      // 「这一条在选中的这一批里」由原语自己贴，不留给各视图各写一遍：漏写不报错，
      // 只是那一格永远不高亮——用户看到的就是"多选做不出来"（真机 2026-09-09：速记
      // 的选择集其实是对的，屏幕上什么也没发生）。
      "aria-selected": selection.paths.has(path),
      onMouseDown: (event: ReactMouseEvent<HTMLElement>) => {
        event.preventDefault();
        const row = event.currentTarget;
        row.focus({ preventScroll: true });
        if (document.activeElement !== row) {
          row
            .closest<HTMLElement>(`[${ROWS_CONTAINER_ATTR}]`)
            ?.focus({ preventScroll: true });
        }
      },
    }),
    contextClick: (path) => setSelection((state) => selectionForContextMenu(state, path)),
    targetsFor: (path) =>
      // 与 `contextClick` 同一条规则（`selectionForContextMenu`），只是它算的是结果集。
      orderedSelection(selectionForContextMenu(selection, path), visible),
  };
}
