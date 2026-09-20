/** 左侧导航：VideoFlow 标识、主页面、分隔线、背景与设置。 */

import { IconConvert, IconDownloadTray, IconHome, IconImage, IconSettings } from "@/components/icons";
import { GlassSurface } from "@/components/glass/GlassSurface";
import appIcon from "@/assets/app-icon.png";
import { useUiStore, type PageId } from "@/stores/uiStore";

interface NavEntry {
  id: PageId;
  label: string;
  icon: React.ReactNode;
}

const PRIMARY: NavEntry[] = [
  { id: "home", label: "首页", icon: <IconHome size={20} /> },
  { id: "downloads", label: "下载记录", icon: <IconDownloadTray size={20} /> },
  { id: "converts", label: "转换记录", icon: <IconConvert size={20} /> },
];

const SECONDARY: NavEntry[] = [
  { id: "background", label: "背景", icon: <IconImage size={20} /> },
  { id: "settings", label: "设置", icon: <IconSettings size={20} /> },
];

export function Sidebar(): React.JSX.Element {
  const page = useUiStore((s) => s.page);
  const setPage = useUiStore((s) => s.setPage);

  const renderItem = (entry: NavEntry) => (
    <button
      key={entry.id}
      type="button"
      className="vf-nav-item"
      aria-current={page === entry.id ? "page" : undefined}
      onClick={() => setPage(entry.id)}
    >
      <span className="vf-nav-item__icon">{entry.icon}</span>
      <span>{entry.label}</span>
    </button>
  );

  return (
    <GlassSurface
      variant="panel"
      className="vf-shell__sidebar"
      id="glass-sidebar"
      role="navigation"
      aria-label="主导航"
      tint={0.58}
      opacity={0.9}
    >
      <div className="vf-sidebar__brand">
        <img src={appIcon} alt="" className="vf-sidebar__logo" width={38} height={38} />
        <span className="vf-sidebar__wordmark">VideoFlow</span>
      </div>

      <nav className="vf-sidebar__nav">{PRIMARY.map(renderItem)}</nav>

      <div className="vf-sidebar__divider" />

      <nav className="vf-sidebar__nav">{SECONDARY.map(renderItem)}</nav>

      <div className="vf-sidebar__tail">
        <blockquote className="vf-sidebar__quote">
          「只要有想见的人，
          <br />
          就不算是一个人了。」
          <cite>—— 天气之子</cite>
        </blockquote>
      </div>
    </GlassSurface>
  );
}