/**
 * 图标资产（唯一一份）：线条图标的 React 组件，桌面与网页共用（TDD-17 §2）——格式工具条、属性面板这类
 * 共享界面画的是同一枚图标，桌面其余界面也从这里取。描边色跟 `currentColor`，尺寸由调用方给。
 */
import type { ReactNode, SVGProps } from "react";

export interface IconProps extends SVGProps<SVGSVGElement> {
  size?: number | string;
}

function createIcon(displayName: string, paths: ReactNode, defaultViewBox = "0 0 24 24") {
  const Component = ({ size = 16, className, ...props }: IconProps) => {
    return (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width={size}
        height={size}
        viewBox={defaultViewBox}
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        className={className}
        {...props}
      >
        {paths}
      </svg>
    );
  };
  Component.displayName = displayName;
  return Component;
}

export const FolderIcon = createIcon(
  "FolderIcon",
  <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />,
);

/**
 * 收件箱（「记录」目的地，PRD-06-058）——托盘。
 *
 * **刻意不再用麦克风**：这个目的地自本批起同时收语音速记与敲进来的文字，
 * 画一支麦克风等于对着一半的内容说谎（也会让用户以为文字捕获框不属于这里）。
 */
export const InboxIcon = createIcon(
  "InboxIcon",
  <>
    <polyline points="22 12 16 12 14 15 10 15 8 12 2 12" />
    <path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z" />
  </>,
);

export const ClockIcon = createIcon(
  "ClockIcon",
  <>
    <circle cx="12" cy="12" r="10" />
    <polyline points="12 6 12 12 16 14" />
  </>,
);

export const MicIcon = createIcon(
  "MicIcon",
  <>
    <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
    <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
    <line x1="12" x2="12" y1="19" y2="22" />
  </>,
);

export const DiscIcon = createIcon(
  "DiscIcon",
  <>
    <circle cx="12" cy="12" r="10" />
    <circle cx="12" cy="12" r="4" />
  </>,
);

export const MessageIcon = createIcon(
  "MessageIcon",
  <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />,
);

export const TrashIcon = createIcon(
  "TrashIcon",
  <>
    <path d="M3 6h18" />
    <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
    <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
  </>,
);

export const ActivityIcon = createIcon("ActivityIcon", <path d="M22 12h-4l-3 9L9 3l-3 9H2" />);

export const SettingsIcon = createIcon(
  "SettingsIcon",
  <>
    <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.1a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
    <circle cx="12" cy="12" r="3" />
  </>,
);

export const SearchIcon = createIcon(
  "SearchIcon",
  <>
    <circle cx="11" cy="11" r="8" />
    <line x1="21" x2="16.65" y1="21" y2="16.65" />
  </>,
);

export const ChevronLeftIcon = createIcon("ChevronLeftIcon", <polyline points="15 18 9 12 15 6" />);

export const ChevronRightIcon = createIcon(
  "ChevronRightIcon",
  <polyline points="9 18 15 12 9 6" />,
);

export const ChevronDownIcon = createIcon("ChevronDownIcon", <polyline points="6 9 12 15 18 9" />);
export const ChevronUpIcon = createIcon("ChevronUpIcon", <polyline points="18 15 12 9 6 15" />);

export const SidebarCloseIcon = createIcon(
  "SidebarCloseIcon",
  <>
    <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
    <line x1="9" x2="9" y1="3" y2="21" />
    <path d="m16 15-3-3 3-3" />
  </>,
);

export const SidebarOpenIcon = createIcon(
  "SidebarOpenIcon",
  <>
    <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
    <line x1="9" x2="9" y1="3" y2="21" />
    <path d="m13 15 3-3-3-3" />
  </>,
);

export const InfoIcon = createIcon(
  "InfoIcon",
  <>
    <circle cx="12" cy="12" r="10" />
    <line x1="12" x2="12" y1="16" y2="12" />
    <line x1="12" x2="12.01" y1="8" y2="8" />
  </>,
);

export const HelpCircleIcon = createIcon(
  "HelpCircleIcon",
  <>
    <circle cx="12" cy="12" r="10" />
    <path d="M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3" />
    <path d="M12 17h.01" />
  </>,
);

export const PlusIcon = createIcon(
  "PlusIcon",
  <>
    <line x1="12" x2="12" y1="5" y2="19" />
    <line x1="5" x2="19" y1="12" y2="12" />
  </>,
);

export const BellIcon = createIcon(
  "BellIcon",
  <>
    <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" />
    <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
  </>,
);

export const SparklesIcon = createIcon(
  "SparklesIcon",
  <>
    <path d="m12 3-1.912 5.813a2 2 0 0 1-1.275 1.275L3 12l5.813 1.912a2 2 0 0 1 1.275 1.275L12 21l1.912-5.813a2 2 0 0 1 1.275-1.275L21 12l-5.813-1.912a2 2 0 0 1-1.275-1.275Z" />
    <path d="m5 3 1 2.5L8.5 6 6 7 5 9.5 4 7 1.5 6 4 6Z" />
    <path d="m19 17 1 2.5 2.5.5-2.5 1-1 2.5-1-2.5-2.5-1 2.5-1Z" />
  </>,
);

export const UserIcon = createIcon(
  "UserIcon",
  <>
    <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />
    <circle cx="12" cy="7" r="4" />
  </>,
);

export const SendIcon = createIcon(
  "SendIcon",
  <>
    <line x1="22" x2="11" y1="2" y2="13" />
    <polygon points="22 2 15 22 11 13 2 9 22 2" />
  </>,
);

export const StopCircleIcon = createIcon(
  "StopCircleIcon",
  <>
    <circle cx="12" cy="12" r="10" />
    <rect x="9" y="9" width="6" height="6" />
  </>,
);

export const CopyIcon = createIcon(
  "CopyIcon",
  <>
    <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
  </>,
);

export const ArrowClockwiseIcon = createIcon(
  "ArrowClockwiseIcon",
  <>
    <path d="M21.5 2v6h-6" />
    <path d="M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67" />
  </>,
);

export const NotebookIcon = createIcon(
  "NotebookIcon",
  <>
    <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z" />
    <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z" />
  </>,
);

export const FileIcon = createIcon(
  "FileIcon",
  <>
    <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
    <path d="M14 2v4a2 2 0 0 0 2 2h4" />
  </>,
);

export const FolderPlusIcon = createIcon(
  "FolderPlusIcon",
  <>
    <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
    <line x1="12" y1="10" x2="12" y2="16" />
    <line x1="9" y1="13" x2="15" y2="13" />
  </>,
);

/**
 * 「从模板新建」。
 *
 * 文件树头部此前用一个**字母 `T`** 占这一格（`template.shortLabel`）——五枚线性图标里
 * 混一个字形，且它套在 `aria-hidden` 的 span 里，于是这颗按钮在读屏下**没有可访问名**。
 * 版式（顶栏 + 两块内容）是模板这件事的通用符号，与树上的文件/文件夹图标不撞。
 */
export const TemplateIcon = createIcon(
  "TemplateIcon",
  <>
    <rect width="18" height="18" x="3" y="3" rx="2" />
    <path d="M3 9h18" />
    <path d="M12 9v12" />
  </>,
);

export const SortIcon = createIcon(
  "SortIcon",
  <>
    <path d="m3 16 4 4 4-4" />
    <path d="M7 20V4" />
    <path d="m21 8-4-4-4 4" />
    <path d="M17 4v16" />
  </>,
);

/** 过滤开关（Lucide filter 漏斗）：文件树「隐藏记录」（PRD-06-052）用——它收窄看得见的
 *  东西而不改数据，漏斗正是这个意思（排序用 `SortIcon`、折叠用 `FoldIcon`，各司其职）。 */
export const FilterIcon = createIcon(
  "FilterIcon",
  <polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3" />,
);

export const FoldIcon = createIcon(
  "FoldIcon",
  <>
    <polyline points="17 11 12 6 7 11" />
    <polyline points="17 18 12 13 7 18" />
  </>,
);

export const StopIcon = createIcon("StopIcon", <rect width="18" height="18" x="3" y="3" rx="2" />);

/** 更多操作（Lucide ellipsis）：HUD 低频动作收进单一溢出入口。 */
export const EllipsisIcon = createIcon(
  "EllipsisIcon",
  <>
    <circle cx="5" cy="12" r="1" fill="currentColor" stroke="none" />
    <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" />
    <circle cx="19" cy="12" r="1" fill="currentColor" stroke="none" />
  </>,
);

export const PauseIcon = createIcon(
  "PauseIcon",
  <>
    <rect x="6" y="4" width="4" height="16" rx="1" />
    <rect x="14" y="4" width="4" height="16" rx="1" />
  </>,
);

export const PlayIcon = createIcon("PlayIcon", <path d="m6 4 14 8-14 8z" />);

export const SpeakIcon = createIcon(
  "SpeakIcon",
  <>
    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
    <path d="M15.54 8.46a5 5 0 0 1 0 7.07" />
    <path d="M19.07 4.93a7 7 0 0 1 0 14.14" />
  </>,
);

export const SpeakingIcon = createIcon(
  "SpeakingIcon",
  <>
    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
    <path d="M16 9a3 3 0 0 1 0 6" />
  </>,
);

export const BookmarkIcon = createIcon(
  "BookmarkIcon",
  <path d="M6 3a2 2 0 0 0-2 2v16l8-5 8 5V5a2 2 0 0 0-2-2z" />,
);

export const MicOffIcon = createIcon(
  "MicOffIcon",
  <>
    <line x1="2" y1="2" x2="22" y2="22" />
    <path d="M9 9v3a3 3 0 0 0 5.12 2.12M15 10.34V5a3 3 0 0 0-5.94-.6" />
    <path d="M17.3 17.3A7 7 0 0 1 5 12v-2M19 10v2a7 7 0 0 1-.11 1.23" />
    <line x1="12" y1="19" x2="12" y2="22" />
  </>,
);

export const CheckIcon = createIcon("CheckIcon", <polyline points="20 6 9 17 4 12" />);

/** 裁剪框：用于「调整取景范围」。齿轮意为「设置/偏好」，用在这里会误导。 */
export const CropIcon = createIcon(
  "CropIcon",
  <>
    <path d="M6 2v14a2 2 0 0 0 2 2h14" />
    <path d="M18 22V8a2 2 0 0 0-2-2H2" />
  </>,
);

export const CameraIcon = createIcon(
  "CameraIcon",
  <>
    <path d="M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3l-2.5-3Z" />
    <circle cx="12" cy="13" r="3" />
  </>,
);

export const PencilIcon = createIcon(
  "PencilIcon",
  <>
    <path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />
    <path d="m15 5 4 4" />
  </>,
);

export const SquareIcon = createIcon(
  "SquareIcon",
  <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />,
);

export const CheckSquareIcon = createIcon(
  "CheckSquareIcon",
  <>
    <polyline points="9 11 12 14 22 4" />
    <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
  </>,
);

export const CloseIcon = createIcon(
  "CloseIcon",
  <>
    <line x1="18" y1="6" x2="6" y2="18" />
    <line x1="6" y1="6" x2="18" y2="18" />
  </>,
);

export const ShieldIcon = createIcon(
  "ShieldIcon",
  <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />,
);

export const CpuIcon = createIcon(
  "CpuIcon",
  <>
    <rect x="4" y="4" width="16" height="16" rx="2" />
    <rect x="9" y="9" width="6" height="6" />
    <path d="M15 2v2M15 20v2M2 15h2M2 9h2M20 15h2M20 9h2M9 2v2M9 20v2" />
  </>,
);

/** 扳手：外部工具（进程外第三方可执行程序）。与 CpuIcon（高级 / 系统）刻意区分。 */
export const WrenchIcon = createIcon(
  "WrenchIcon",
  <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />,
);

/** 词条卡片：个人词典（原始词 → 正确形式的对照表）。 */
export const GlossaryIcon = createIcon(
  "GlossaryIcon",
  <>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M12 4v16M6 9h3M6 13h3M15 9h3M15 13h3" />
  </>,
);

export const DownloadIcon = createIcon(
  "DownloadIcon",
  <>
    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
    <polyline points="7 10 12 15 17 10" />
    <line x1="12" x2="12" y1="15" y2="3" />
  </>,
);

export const GlobeIcon = createIcon(
  "GlobeIcon",
  <>
    <circle cx="12" cy="12" r="10" />
    <path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20" />
    <path d="M2 12h20" />
  </>,
);

export const HardDriveIcon = createIcon(
  "HardDriveIcon",
  <>
    <line x1="22" x2="2" y1="12" y2="12" />
    <path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" />
    <line x1="6" x2="6.01" y1="16" y2="16" />
    <line x1="10" x2="10.01" y1="16" y2="16" />
  </>,
);

export const KeyboardIcon = createIcon(
  "KeyboardIcon",
  <>
    <rect x="2" y="6" width="20" height="12" rx="2" />
    <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M6 14h.01M18 14h.01M9 14h6" />
  </>,
);

export const LanguagesIcon = createIcon(
  "LanguagesIcon",
  <>
    <path d="m5 8 6 6" />
    <path d="m4 14 6-6 2-3" />
    <path d="M2 5h12" />
    <path d="M7 2h1" />
    <path d="m22 22-5-10-5 10" />
    <path d="M14 18h6" />
  </>,
);

/** 实时字幕（屏幕文本轨道，避免与翻译入口使用同一图标）。 */
export const CaptionsIcon = createIcon(
  "CaptionsIcon",
  <>
    <rect x="3" y="5" width="18" height="14" rx="3" />
    <path d="M7 10h4M7 14h7M15 10h2M16 14h1" />
  </>,
);

/** 锁：设置里「锚点不可隐藏」那一行的开关位（PRD-07-003）。 */
export const LockIcon = createIcon(
  "LockIcon",
  <>
    <rect x="4" y="11" width="16" height="10" rx="2" />
    <path d="M8 11V7a4 4 0 0 1 8 0v4" />
  </>,
);

/** 便签（右下角折起）：「速记」空间的图标（PRD-07-066——不用麦克风、不用收件托盘）。 */
export const StickyNoteIcon = createIcon(
  "StickyNoteIcon",
  <>
    <path d="M5 3h14a2 2 0 0 1 2 2v9l-7 7H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z" />
    <path d="M14 21v-5a2 2 0 0 1 2-2h5" />
  </>,
);

/** 退出全屏/沉浸（四角向内箭头，Lucide shrink）。 */
export const CompressIcon = createIcon(
  "CompressIcon",
  <>
    <path d="m15 15 6 6m-6-6v4.8m0-4.8h4.8" />
    <path d="M9 19.8V15m0 0H4.2M9 15l-6 6" />
    <path d="M15 4.2V9m0 0h4.8M15 9l6-6" />
    <path d="M9 4.2V9m0 0H4.2M9 9 3 3" />
  </>,
);

/** 固定位置（图钉，Lucide pin）。 */
export const PinIcon = createIcon(
  "PinIcon",
  <>
    <path d="M12 17v5" />
    <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" />
  </>,
);

/** 图钉·已固定：同一字形实心（`aria-pressed` 之外的第二线索——此前固定与否只差一层底色）。 */
export const PinFilledIcon = createIcon(
  "PinFilledIcon",
  <>
    <path d="M12 17v5" />
    <path
      fill="currentColor"
      d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"
    />
  </>,
);

/** 联网指示（音频离开本机，Lucide cloud-upload）：此前是一枚 8px 三角，读作报警。 */
export const CloudUploadIcon = createIcon(
  "CloudUploadIcon",
  <>
    <path d="M4 14.899A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 2.5 8.242" />
    <path d="M12 12v9" />
    <path d="m16 16-4-4-4 4" />
  </>,
);

/** 展开为标准窗口（对角向外箭头，Lucide maximize-2）。 */
export const ExpandIcon = createIcon(
  "ExpandIcon",
  <>
    <polyline points="15 3 21 3 21 9" />
    <polyline points="9 21 3 21 3 15" />
    <line x1="21" y1="3" x2="14" y2="10" />
    <line x1="3" y1="21" x2="10" y2="14" />
  </>,
);

/** 源码模式（Lucide code-xml）：Markdown 原文编辑，无就地渲染装饰。 */
export const CodeIcon = createIcon(
  "CodeIcon",
  <>
    <polyline points="16 18 22 12 16 6" />
    <polyline points="8 6 2 12 8 18" />
  </>,
);

/** 阅读模式（Lucide book-open）：只读渲染视图。 */
export const BookOpenIcon = createIcon(
  "BookOpenIcon",
  <>
    <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z" />
    <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z" />
  </>,
);

/** 图片文件（Lucide image）：树内图片附件类型图标（PRD-06-007）。 */
export const ImageIcon = createIcon(
  "ImageIcon",
  <>
    <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
    <circle cx="9" cy="9" r="2" />
    <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
  </>,
);

/** 音频文件（Lucide music）：树内音频附件类型图标（PRD-06-007）。 */
export const MusicIcon = createIcon(
  "MusicIcon",
  <>
    <path d="M9 18V5l12-2v13" />
    <circle cx="6" cy="18" r="3" />
    <circle cx="18" cy="16" r="3" />
  </>,
);

/**
 * 视频/媒体（Lucide film）：树内视频附件类型图标（PRD-06-007）与
 * 「转写」目的地图标（导入媒体的产物，与录音的碟片图标形成来源区分，PRD-05-030）共用。
 */
export const FilmIcon = createIcon(
  "FilmIcon",
  <>
    <rect width="18" height="18" x="3" y="3" rx="2" />
    <path d="M7 3v18M17 3v18M3 7.5h4M3 12h18M3 16.5h4M17 7.5h4M17 16.5h4" />
  </>,
);

/** 应用（Lucide layout-grid）：活动栏贴底的「应用」目的地（ADR-165 ⑤）。 */
export const LayoutGridIcon = createIcon(
  "LayoutGridIcon",
  <>
    <rect width="7" height="7" x="3" y="3" rx="1" />
    <rect width="7" height="7" x="14" y="3" rx="1" />
    <rect width="7" height="7" x="14" y="14" rx="1" />
    <rect width="7" height="7" x="3" y="14" rx="1" />
  </>,
);

/** 文档/PDF 文件（Lucide file-text）：树内文档附件类型图标（PRD-06-007）。 */
export const FileTextIcon = createIcon(
  "FileTextIcon",
  <>
    <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z" />
    <path d="M14 2v5h5M16 13H8M16 17H8M10 9H8" />
  </>,
);

/** 提词器（Lucide scroll-text）：卷起的稿子，活动栏上的「提词器」（ADR-176 ①）。 */
export const ScrollTextIcon = createIcon(
  "ScrollTextIcon",
  <>
    <path d="M15 12h-5M15 8h-5M19 17V5a2 2 0 0 0-2-2H4" />
    <path d="M8 21h12a2 2 0 0 0 2-2v-1a1 1 0 0 0-1-1H11a1 1 0 0 0-1 1v1a2 2 0 1 1-4 0V5a2 2 0 1 0-4 0v2a1 1 0 0 0 1 1h3" />
  </>,
);

/** 弹出独立窗口（Lucide external-link 变体）：移到新窗口（PRD-07-036）。 */
export const ExternalWindowIcon = createIcon(
  "ExternalWindowIcon",
  <>
    <path d="M15 3h6v6" />
    <path d="M10 14 21 3" />
    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
  </>,
);

/** 分屏（Lucide columns-2）：主区左右并排（PRD-07-036）。 */
export const SplitIcon = createIcon(
  "SplitIcon",
  <>
    <rect x="3" y="3" width="18" height="18" rx="2" />
    <line x1="12" y1="3" x2="12" y2="21" />
  </>,
);

/** 临时对话（幽灵/无痕，Lucide ghost）：不落盘、不进历史。 */
export const GhostIcon = createIcon(
  "GhostIcon",
  <>
    <path d="M9 10h.01" />
    <path d="M15 10h.01" />
    <path d="M12 2a8 8 0 0 0-8 8v12l3-3 2.5 2.5L12 21l2.5 2.5L17 19l3 3V10a8 8 0 0 0-8-8z" />
  </>,
);

/** 表格（Lucide table）：编辑器插入表格入口（PRD-06-011 表格 Live Preview）。 */
export const TableIcon = createIcon(
  "TableIcon",
  <>
    <rect x="3" y="3" width="18" height="18" rx="2" />
    <line x1="3" y1="9" x2="21" y2="9" />
    <line x1="3" y1="15" x2="21" y2="15" />
    <line x1="12" y1="3" x2="12" y2="21" />
  </>,
);

/* ─── 格式工具栏 / 编辑器右键菜单（PRD-06-014，Lucide 系）─────────────────── */

export const BoldIcon = createIcon(
  "BoldIcon",
  <path d="M6 12h9a4 4 0 0 1 0 8H7a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h7a4 4 0 0 1 0 8" />,
);

export const ItalicIcon = createIcon(
  "ItalicIcon",
  <>
    <line x1="19" y1="4" x2="10" y2="4" />
    <line x1="14" y1="20" x2="5" y2="20" />
    <line x1="15" y1="4" x2="9" y2="20" />
  </>,
);

export const StrikethroughIcon = createIcon(
  "StrikethroughIcon",
  <>
    <path d="M16 4H9a3 3 0 0 0-2.83 4" />
    <path d="M14 12a4 4 0 0 1 0 8H6" />
    <line x1="4" y1="12" x2="20" y2="12" />
  </>,
);

export const HeadingIcon = createIcon(
  "HeadingIcon",
  <>
    <path d="M6 4v16" />
    <path d="M18 4v16" />
    <path d="M6 12h12" />
  </>,
);

/**
 * 处理队列入口（托盘 + 两条任务线）。此前入口用的是 `ArrowClockwiseIcon`——与面板
 * 内「重试」同一枚字形，空闲时看起来就是一颗刷新按钮（真机 2026-09-07）。
 */
export const QueueIcon = createIcon(
  "QueueIcon",
  <>
    <path d="M4 14v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
    <polyline points="4 14 8 14 10 17 14 17 16 14 20 14" />
    <line x1="7" y1="5" x2="17" y2="5" />
    <line x1="7" y1="9" x2="17" y2="9" />
  </>,
);

export const ListIcon = createIcon(
  "ListIcon",
  <>
    <line x1="8" y1="6" x2="21" y2="6" />
    <line x1="8" y1="12" x2="21" y2="12" />
    <line x1="8" y1="18" x2="21" y2="18" />
    <line x1="3" y1="6" x2="3.01" y2="6" />
    <line x1="3" y1="12" x2="3.01" y2="12" />
    <line x1="3" y1="18" x2="3.01" y2="18" />
  </>,
);

export const ListOrderedIcon = createIcon(
  "ListOrderedIcon",
  <>
    <line x1="10" y1="6" x2="21" y2="6" />
    <line x1="10" y1="12" x2="21" y2="12" />
    <line x1="10" y1="18" x2="21" y2="18" />
    <path d="M4 6h1v4" />
    <path d="M4 10h2" />
    <path d="M6 18H4c0-1 2-2 2-3s-1-1.5-2-1" />
  </>,
);

export const ListTodoIcon = createIcon(
  "ListTodoIcon",
  <>
    <rect x="3" y="5" width="6" height="6" rx="1" />
    <path d="m3 17 2 2 4-4" />
    <line x1="13" y1="6" x2="21" y2="6" />
    <line x1="13" y1="12" x2="21" y2="12" />
    <line x1="13" y1="18" x2="21" y2="18" />
  </>,
);

/**
 * 引用（Lucide quote，一对引号）。
 *
 * 此前是 Lucide text-quote（左侧竖条 + 缩进的行）——它长得和「增加缩进」几乎一样，
 * 真机评审时被当成缩进钮（2026-09-02）。引号本身没有第二种读法。
 */
export const QuoteIcon = createIcon(
  "QuoteIcon",
  <>
    <path d="M3 21c3 0 7-1 7-8V5c0-1.25-.756-2.017-2-2H4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2 1 0 1 0 1 1v1c0 1-1 2-2 2s-1 .008-1 1.031V20c0 1 0 1 1 1z" />
    <path d="M15 21c3 0 7-1 7-8V5c0-1.25-.757-2.017-2-2h-4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2h.75c0 2.25.25 4-2.75 4v3c0 1 0 1 1 1z" />
  </>,
);

/**
 * 标注块（Lucide message-square-warning：带感叹号的方框）。
 *
 * 工具栏此前借用 `InfoIcon`（圆圈 i）——那是「信息 / 说明」的通用记号，右侧面板的
 * 「笔记信息」页签、搜索页的提示都在用它；同一个图标在同一屏既是"查看信息"又是
 * "插入标注块"，两处都读不准。
 */
export const CalloutIcon = createIcon(
  "CalloutIcon",
  <>
    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
    <path d="M12 7v2" />
    <path d="M12 13h.01" />
  </>,
);

export const LinkIcon = createIcon(
  "LinkIcon",
  <>
    <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
    <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
  </>,
);

/** wiki 链接（双方括号意象，Lunarca 自绘）。 */
/** wiki 链接：双方括号 `[[ ]]`——就是它的语法本身；单层 `[ ]` 与普通链接分不开。 */
export const WikiLinkIcon = createIcon(
  "WikiLinkIcon",
  <>
    <path d="M6 4H3v16h3" />
    <path d="M11 4H8v16h3" />
    <path d="M13 4h3v16h-3" />
    <path d="M18 4h3v16h-3" />
  </>,
);

export const MinusIcon = createIcon("MinusIcon", <line x1="5" y1="12" x2="19" y2="12" />);

export const CodeSquareIcon = createIcon(
  "CodeSquareIcon",
  <>
    <rect x="3" y="3" width="18" height="18" rx="2" />
    <path d="m10 9-2 3 2 3" />
    <path d="m14 9 2 3-2 3" />
  </>,
);

/** 专注模式（Lucide focus）：淡化非当前段落（PRD-06-014，Typora 对标）。 */
export const FocusIcon = createIcon(
  "FocusIcon",
  <>
    <circle cx="12" cy="12" r="3" />
    <path d="M3 7V5a2 2 0 0 1 2-2h2" />
    <path d="M17 3h2a2 2 0 0 1 2 2v2" />
    <path d="M21 17v2a2 2 0 0 1-2 2h-2" />
    <path d="M7 21H5a2 2 0 0 1-2-2v-2" />
  </>,
);

/** 关系图谱（Lucide waypoints）：知识网络视图入口（PRD-06-034，Obsidian Graph 对标）。 */
export const GraphIcon = createIcon(
  "GraphIcon",
  <>
    <circle cx="12" cy="4.5" r="2.5" />
    <path d="m10.2 6.3-3.9 3.9" />
    <circle cx="4.5" cy="12" r="2.5" />
    <path d="M7 12h10" />
    <circle cx="19.5" cy="12" r="2.5" />
    <path d="m13.8 17.7 3.9-3.9" />
    <circle cx="12" cy="19.5" r="2.5" />
  </>,
);

/** AI 关系标签（Lucide tags）：LLM 生成/更新关系标签入口（PRD-06-035）。 */
export const TagsIcon = createIcon(
  "TagsIcon",
  <>
    <path d="m15 5 6.3 6.3a2.4 2.4 0 0 1 0 3.4L17 19" />
    <path d="M9.586 5.586A2 2 0 0 0 8.172 5H3a1 1 0 0 0-1 1v5.172a2 2 0 0 0 .586 1.414L8.29 18.29a2.426 2.426 0 0 0 3.42 0l3.58-3.58a2.426 2.426 0 0 0 0-3.42z" />
    <circle cx="6.5" cy="9.5" r=".5" fill="currentColor" />
  </>,
);

/** 在光标处拆分字幕条目（Lucide scissors）：一条里有两位说话人时切开（PRD-04-056）。 */
export const ScissorsIcon = createIcon(
  "ScissorsIcon",
  <>
    <circle cx="6" cy="6" r="3" />
    <circle cx="6" cy="18" r="3" />
    <path d="M8.12 8.12 20 20" />
    <path d="M20 4 8.12 15.88" />
  </>,
);

/** 与上一条合并（Lucide merge）：拆分的逆操作，也用于还原被 ASR 切碎的半句（PRD-04-056）。 */
export const MergeIcon = createIcon(
  "MergeIcon",
  <>
    <path d="m8 6 4-4 4 4" />
    <path d="M12 2v10.3a4 4 0 0 1-1.172 2.872L4 22" />
    <path d="m20 22-5-5" />
  </>,
);

/** 拖动把手（Lucide grip-vertical）：标示"按住这里可以拖"（悬浮小窗标题栏，PRD-04-058）。 */
export const GripIcon = createIcon(
  "GripIcon",
  <>
    <circle cx="9" cy="6" r="1" fill="currentColor" />
    <circle cx="9" cy="12" r="1" fill="currentColor" />
    <circle cx="9" cy="18" r="1" fill="currentColor" />
    <circle cx="15" cy="6" r="1" fill="currentColor" />
    <circle cx="15" cy="12" r="1" fill="currentColor" />
    <circle cx="15" cy="18" r="1" fill="currentColor" />
  </>,
);

/** 查找替换（Lucide replace）：两个错位方块 = 用一个换掉另一个（PRD-06-014）。 */
export const ReplaceIcon = createIcon(
  "ReplaceIcon",
  <>
    <path d="M14 4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2" />
    <path d="M4 14a2 2 0 0 0-2 2v4a2 2 0 0 0 2 2h4a2 2 0 0 0 2-2" />
    <rect x="2" y="2" width="8" height="8" rx="2" />
    <rect x="14" y="14" width="8" height="8" rx="2" />
  </>,
);

/** 悬浮小窗（Lucide picture-in-picture-2）：视频画面浮于转写之上（PRD-04-058）。 */
export const PictureInPictureIcon = createIcon(
  "PictureInPictureIcon",
  <>
    <path d="M21 9V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h5" />
    <rect width="10" height="7" x="12" y="13" rx="2" />
  </>,
);

/** 听诊器：知识体检（PRD-16 §3.7）的专属图标——一入口一个图标，绝不复用。 */
export const StethoscopeIcon = createIcon(
  "StethoscopeIcon",
  <>
    <path d="M4.8 2.3A.3.3 0 1 0 5 2H4a2 2 0 0 0-2 2v5a6 6 0 0 0 6 6 6 6 0 0 0 6-6V4a2 2 0 0 0-2-2h-1a.2.2 0 1 0 .3.3" />
    <path d="M8 15v1a6 6 0 0 0 6 6 6 6 0 0 0 6-6v-4" />
    <circle cx="20" cy="10" r="2" />
  </>,
);
