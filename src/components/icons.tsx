/**
 * 图标集：内联 SVG，线性风格，与预览图一致。
 * 统一 24×24 视图框、round linecap，颜色继承 currentColor。
 */

interface IconProps {
  size?: number;
  className?: string;
  strokeWidth?: number;
}

function Base({
  size = 20,
  className,
  strokeWidth = 1.7,
  children,
  viewBox = "0 0 24 24",
}: IconProps & { children: React.ReactNode; viewBox?: string }): React.JSX.Element {
  return (
    <svg
      width={size}
      height={size}
      viewBox={viewBox}
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

export const IconHome = (p: IconProps) => (
  <Base {...p}>
    <path d="M3.5 10.2 12 3.6l8.5 6.6" />
    <path d="M5.2 9.1V19a1.4 1.4 0 0 0 1.4 1.4h10.8A1.4 1.4 0 0 0 18.8 19V9.1" />
    <path d="M9.8 20.4v-5.2a2.2 2.2 0 0 1 4.4 0v5.2" />
  </Base>
);

export const IconDownloadTray = (p: IconProps) => (
  <Base {...p}>
    <path d="M12 3.8v10.4" />
    <path d="m7.8 10.4 4.2 4.2 4.2-4.2" />
    <path d="M4.4 16.6v1.8a2.2 2.2 0 0 0 2.2 2.2h10.8a2.2 2.2 0 0 0 2.2-2.2v-1.8" />
  </Base>
);

export const IconConvert = (p: IconProps) => (
  <Base {...p}>
    <path d="M20 11.2a8 8 0 0 0-13.9-4.4L4 8.9" />
    <path d="M4 4.6v4.6h4.6" />
    <path d="M4 12.8a8 8 0 0 0 13.9 4.4l2.1-2.1" />
    <path d="M20 19.4v-4.6h-4.6" />
  </Base>
);

export const IconImage = (p: IconProps) => (
  <Base {...p}>
    <rect x="3.4" y="4.6" width="17.2" height="14.8" rx="2.4" />
    <circle cx="8.6" cy="9.8" r="1.7" />
    <path d="m3.9 17.4 4.5-4.3a2 2 0 0 1 2.8 0l3 2.9" />
    <path d="m13.2 14.7 2.1-2a2 2 0 0 1 2.8 0l2.4 2.3" />
  </Base>
);

export const IconSettings = (p: IconProps) => (
  <Base {...p}>
    <circle cx="12" cy="12" r="3.1" />
    <path d="M19.2 14.4a1.6 1.6 0 0 0 .32 1.77l.06.06a1.94 1.94 0 1 1-2.74 2.74l-.06-.06a1.6 1.6 0 0 0-1.77-.32 1.6 1.6 0 0 0-.97 1.47v.17a1.94 1.94 0 1 1-3.88 0v-.09a1.6 1.6 0 0 0-1.05-1.47 1.6 1.6 0 0 0-1.77.32l-.06.06a1.94 1.94 0 1 1-2.74-2.74l.06-.06a1.6 1.6 0 0 0 .32-1.77 1.6 1.6 0 0 0-1.47-.97h-.17a1.94 1.94 0 1 1 0-3.88h.09a1.6 1.6 0 0 0 1.47-1.05 1.6 1.6 0 0 0-.32-1.77l-.06-.06a1.94 1.94 0 1 1 2.74-2.74l.06.06a1.6 1.6 0 0 0 1.77.32h.08a1.6 1.6 0 0 0 .97-1.47v-.17a1.94 1.94 0 1 1 3.88 0v.09a1.6 1.6 0 0 0 .97 1.47 1.6 1.6 0 0 0 1.77-.32l.06-.06a1.94 1.94 0 1 1 2.74 2.74l-.06.06a1.6 1.6 0 0 0-.32 1.77v.08a1.6 1.6 0 0 0 1.47.97h.17a1.94 1.94 0 1 1 0 3.88h-.09a1.6 1.6 0 0 0-1.47.97Z" />
  </Base>
);

export const IconLink = (p: IconProps) => (
  <Base {...p}>
    <path d="M10.1 13.9a3.9 3.9 0 0 0 5.5 0l2.6-2.6a3.9 3.9 0 1 0-5.5-5.5l-1.1 1.1" />
    <path d="M13.9 10.1a3.9 3.9 0 0 0-5.5 0l-2.6 2.6a3.9 3.9 0 1 0 5.5 5.5l1.1-1.1" />
  </Base>
);

export const IconClock = (p: IconProps) => (
  <Base {...p}>
    <circle cx="12" cy="12" r="8.2" />
    <path d="M12 7.6V12l2.9 1.8" />
  </Base>
);

export const IconDimensions = (p: IconProps) => (
  <Base {...p}>
    <path d="M4 8.4V5.6A1.6 1.6 0 0 1 5.6 4h2.8" />
    <path d="M20 8.4V5.6A1.6 1.6 0 0 0 18.4 4h-2.8" />
    <path d="M4 15.6v2.8A1.6 1.6 0 0 0 5.6 20h2.8" />
    <path d="M20 15.6v2.8A1.6 1.6 0 0 1 18.4 20h-2.8" />
    <rect x="8.8" y="8.8" width="6.4" height="6.4" rx="1.2" />
  </Base>
);

export const IconFileSize = (p: IconProps) => (
  <Base {...p}>
    <rect x="3" y="6.6" width="18" height="10.8" rx="2.2" />
    <path d="M8.2 10.6v3.8" />
    <path d="M15.8 10.6v3.8" />
    <path d="M8.2 12.6h7.6" />
  </Base>
);

export const IconSource = (p: IconProps) => (
  <Base {...p}>
    <rect x="3.2" y="5" width="17.6" height="15" rx="2.4" />
    <path d="M3.2 9.6h17.6" />
    <path d="M7.6 3.2v3.2" />
    <path d="M16.4 3.2v3.2" />
  </Base>
);

export const IconPlay = (p: IconProps) => (
  <Base {...p}>
    <path d="M8.4 5.9v12.2l9.6-6.1z" fill="currentColor" stroke="none" />
  </Base>
);

export const IconPause = (p: IconProps) => (
  <Base {...p}>
    <rect x="7.6" y="5.4" width="3.2" height="13.2" rx="1.3" fill="currentColor" stroke="none" />
    <rect x="13.2" y="5.4" width="3.2" height="13.2" rx="1.3" fill="currentColor" stroke="none" />
  </Base>
);

export const IconClose = (p: IconProps) => (
  <Base {...p}>
    <path d="M6.6 6.6l10.8 10.8" />
    <path d="M17.4 6.6 6.6 17.4" />
  </Base>
);

export const IconCheck = (p: IconProps) => (
  <Base {...p}>
    <path d="m5.4 12.6 4.4 4.4 8.8-9.6" />
  </Base>
);

export const IconFolder = (p: IconProps) => (
  <Base {...p}>
    <path d="M3.6 7.4a2 2 0 0 1 2-2h3.1l1.8 2.2h7.9a2 2 0 0 1 2 2v7.6a2 2 0 0 1-2 2h-12.8a2 2 0 0 1-2-2z" />
  </Base>
);

export const IconChevronDown = (p: IconProps) => (
  <Base {...p}>
    <path d="m6.4 9.6 5.6 5.2 5.6-5.2" />
  </Base>
);

export const IconChevronRight = (p: IconProps) => (
  <Base {...p}>
    <path d="m9.6 6.4 5.2 5.6-5.2 5.6" />
  </Base>
);

export const IconMinimize = (p: IconProps) => (
  <Base {...p}>
    <path d="M6 12h12" />
  </Base>
);

export const IconMaximize = (p: IconProps) => (
  <Base {...p}>
    <rect x="6" y="6" width="12" height="12" rx="2" />
  </Base>
);

export const IconRestore = (p: IconProps) => (
  <Base {...p}>
    <rect x="4.6" y="7.8" width="10" height="10" rx="1.8" />
    <path d="M9.4 5.4h7.2a2 2 0 0 1 2 2v7.2" />
  </Base>
);

export const IconRefresh = (p: IconProps) => (
  <Base {...p}>
    <path d="M20.2 11.4a8.2 8.2 0 0 0-14.2-4.6L3.8 9" />
    <path d="M3.8 4.6V9h4.4" />
    <path d="M3.8 12.6a8.2 8.2 0 0 0 14.2 4.6l2.2-2.2" />
    <path d="M20.2 19.4V15h-4.4" />
  </Base>
);

export const IconTrash = (p: IconProps) => (
  <Base {...p}>
    <path d="M4.6 6.8h14.8" />
    <path d="M9.4 6.8V5.2a1.4 1.4 0 0 1 1.4-1.4h2.4a1.4 1.4 0 0 1 1.4 1.4v1.6" />
    <path d="M6.6 6.8l.9 11.4a1.8 1.8 0 0 0 1.8 1.6h5.4a1.8 1.8 0 0 0 1.8-1.6l.9-11.4" />
    <path d="M10.4 10.6v5.6" />
    <path d="M13.6 10.6v5.6" />
  </Base>
);

export const IconSearch = (p: IconProps) => (
  <Base {...p}>
    <circle cx="11" cy="11" r="6.4" />
    <path d="m15.8 15.8 3.6 3.6" />
  </Base>
);

export const IconExternal = (p: IconProps) => (
  <Base {...p}>
    <path d="M14.2 4.4h5.4v5.4" />
    <path d="M19.6 4.4 11 13" />
    <path d="M18 14.4v3.8a1.8 1.8 0 0 1-1.8 1.8H5.8A1.8 1.8 0 0 1 4 18.2V7.8A1.8 1.8 0 0 1 5.8 6h3.8" />
  </Base>
);

export const IconUpload = (p: IconProps) => (
  <Base {...p}>
    <path d="M12 16.4V5.6" />
    <path d="m7.8 9.8 4.2-4.2 4.2 4.2" />
    <path d="M4.4 16.6v1.8a2.2 2.2 0 0 0 2.2 2.2h10.8a2.2 2.2 0 0 0 2.2-2.2v-1.8" />
  </Base>
);

/** 置顶：把任务提到队列最前 */
export const IconArrowUp = (p: IconProps) => (
  <Base {...p}>
    <path d="M12 19.4V5.2" />
    <path d="m6.2 11 5.8-5.8 5.8 5.8" />
  </Base>
);

export const IconLayers = (p: IconProps) => (
  <Base {...p}>
    <path d="m12 3.6 8.4 4.6-8.4 4.6-8.4-4.6z" />
    <path d="m4.6 12.6 7.4 4 7.4-4" />
    <path d="m4.6 16.6 7.4 4 7.4-4" />
  </Base>
);

export const IconInfo = (p: IconProps) => (
  <Base {...p}>
    <circle cx="12" cy="12" r="8.2" />
    <path d="M12 11v5.2" />
    <path d="M12 7.9h.01" />
  </Base>
);

export const IconWarning = (p: IconProps) => (
  <Base {...p}>
    <path d="M10.3 4.3 2.8 17.4a1.9 1.9 0 0 0 1.6 2.9h15.2a1.9 1.9 0 0 0 1.6-2.9L13.7 4.3a1.9 1.9 0 0 0-3.4 0Z" />
    <path d="M12 9.4v4.2" />
    <path d="M12 17h.01" />
  </Base>
);

/** 品牌标识改用 docs/图标.png（见 src/assets/app-icon.png），此处不再内联绘制 */