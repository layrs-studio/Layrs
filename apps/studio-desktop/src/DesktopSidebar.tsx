import { Sidebar } from "@layrs/ui";
import { ShortcutFooter } from "./DesktopSettingsView";
import type { DesktopPage } from "./desktopTypes";
import type { DesktopShortcutSettings, LocalSpaceSummary } from "./tauri";

interface DesktopSidebarProps {
  availableCount: number;
  localSpaces: LocalSpaceSummary[];
  page: DesktopPage;
  selectedLocalSpace: LocalSpaceSummary | null;
  shortcuts: DesktopShortcutSettings;
}

export function DesktopSidebar({
  availableCount,
  localSpaces,
  page,
  selectedLocalSpace,
  shortcuts
}: DesktopSidebarProps) {
  const isSpacePage = page === "local" || page === "spaceSettings" || page === "weaves";
  const localSpaceItems = localSpaces.map((space) => ({
    id: `desktop-local:${encodeURIComponent(space.localSpaceId)}`,
    label: space.name,
    eyebrow: space.state === "draft" ? "Draft Space" : "Local Space",
    isActive: isSpacePage && selectedLocalSpace?.localSpaceId === space.localSpaceId,
    meta: space.layers.length > 1 ? `${space.layers.length}` : undefined
  }));

  return (
    <Sidebar
      items={[
        {
          id: "desktop-available",
          label: "Distant",
          eyebrow: "Server",
          isActive: page === "available",
          meta: `${availableCount}`
        },
        ...localSpaceItems,
        {
          id: "desktop-draft",
          label: "Local setup",
          eyebrow: "Offline",
          isActive: page === "draft"
        },
        {
          id: "desktop-settings",
          label: "Settings",
          eyebrow: "Device",
          isActive: page === "settings"
        }
      ]}
      footer={<ShortcutFooter hasLocalSpace={Boolean(selectedLocalSpace)} shortcuts={shortcuts} />}
    />
  );
}
