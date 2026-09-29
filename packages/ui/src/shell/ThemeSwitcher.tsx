import { Monitor, Moon, Sun } from "lucide-react";
import { SegmentedControl } from "../components/SegmentedControl";
import { useAppearance, type ThemePreference } from "../foundation/appearance";

export function ThemeSwitcher({ size = "sm" }: { size?: "sm" | "md" }) {
  const { appearance, setAppearance } = useAppearance();
  return (
    <SegmentedControl<ThemePreference>
      label="Thème"
      size={size}
      value={appearance.theme}
      onChange={(theme) => setAppearance({ theme })}
      options={[
        { value: "dark", label: "Sombre", icon: <Moon aria-hidden size={14} /> },
        { value: "light", label: "Clair", icon: <Sun aria-hidden size={14} /> },
        { value: "system", label: "Système", icon: <Monitor aria-hidden size={14} /> },
      ]}
    />
  );
}
