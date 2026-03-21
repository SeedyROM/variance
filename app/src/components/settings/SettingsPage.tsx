import { User, Globe, Database, Palette, X, Keyboard } from "lucide-react";
import { useEffect } from "react";
import { IconButton } from "../ui/IconButton";
import { AccountSection } from "./AccountSection";
import { NetworkSection } from "./NetworkSection";
import { StorageSection } from "./StorageSection";
import { AppearanceSection } from "./AppearanceSection";
import { useAppStore, type SettingsSection } from "../../stores/appStore";
import { cn } from "../../utils/cn";

const sections: { key: SettingsSection; label: string; icon: React.ReactNode }[] = [
  { key: "account", label: "Account", icon: <User className="h-4 w-4" /> },
  { key: "network", label: "Network", icon: <Globe className="h-4 w-4" /> },
  { key: "storage", label: "Storage", icon: <Database className="h-4 w-4" /> },
  { key: "appearance", label: "Appearance", icon: <Palette className="h-4 w-4" /> },
];

const sectionComponents: Record<SettingsSection, React.FC> = {
  account: AccountSection,
  network: NetworkSection,
  storage: StorageSection,
  appearance: AppearanceSection,
};

export function SettingsPage() {
  const activeSection = useAppStore((s) => s.settingsSection);
  const setSection = useAppStore((s) => s.setSettingsSection);
  const closeSettings = useAppStore((s) => s.closeSettings);

  // Close on Escape
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeSettings();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [closeSettings]);

  const ActiveComponent = sectionComponents[activeSection];

  return (
    <div className="fixed inset-0 z-50 flex bg-surface-100 dark:bg-surface-950">
      {/* Sidebar */}
      <nav className="flex w-56 shrink-0 flex-col border-r border-surface-200 bg-surface-50 dark:border-surface-800 dark:bg-surface-900">
        {/* Spacer — clears macOS traffic lights */}
        <div className="h-7 shrink-0" />

        <div className="flex items-center justify-between border-b border-surface-200 px-4 py-3 dark:border-surface-800">
          <h2 className="font-semibold text-surface-900 dark:text-surface-50 cursor-default">
            Settings
          </h2>
          {/* Invisible spacer matching IconButton height so header aligns with Messages header */}
          <div className="h-4 w-0 p-1.5 box-content" aria-hidden="true" />
        </div>

        <div className="flex-1 space-y-0.5 px-2 pt-2">
          {sections.map((s) => (
            <button
              key={s.key}
              onClick={() => setSection(s.key)}
              className={cn(
                "flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-sm transition-colors cursor-pointer",
                activeSection === s.key
                  ? "bg-surface-200 text-surface-900 dark:bg-surface-800 dark:text-surface-50"
                  : "text-surface-600 hover:bg-surface-200/60 hover:text-surface-900 dark:text-surface-400 dark:hover:bg-surface-800/60 dark:hover:text-surface-200"
              )}
            >
              {s.icon}
              {s.label}
            </button>
          ))}
        </div>

        {/* Keyboard hint */}
        <div className="px-4 py-3 border-t border-surface-200 dark:border-surface-800">
          <p className="flex items-center gap-1.5 text-xs text-surface-400">
            <Keyboard className="h-3 w-3" />
            <kbd className="rounded bg-surface-200 px-1 py-0.5 font-mono text-xs dark:bg-surface-800">
              Esc
            </kbd>
            to close
          </p>
        </div>
      </nav>

      {/* Content */}
      <div className="flex-1 overflow-auto">
        {/* Top bar with close button */}
        <div className="sticky top-0 z-10 flex items-center justify-end p-3 bg-surface-100/80 dark:bg-surface-950/80 backdrop-blur-sm">
          <IconButton onClick={closeSettings} title="Close settings">
            <X className="h-5 w-5" />
          </IconButton>
        </div>

        {/* Section content */}
        <div className="mx-auto max-w-2xl px-8 pb-12">
          <ActiveComponent />
        </div>
      </div>
    </div>
  );
}
