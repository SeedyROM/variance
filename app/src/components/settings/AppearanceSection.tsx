import { Moon, Sun, Monitor } from "lucide-react";
import { useTheme, type Theme } from "../../hooks/useTheme";
import { cn } from "../../utils/cn";

const options: { value: Theme; icon: React.ReactNode; label: string; description: string }[] = [
  {
    value: "light",
    icon: <Sun className="h-5 w-5" />,
    label: "Light",
    description: "Always use light theme",
  },
  {
    value: "system",
    icon: <Monitor className="h-5 w-5" />,
    label: "System",
    description: "Follow your OS setting",
  },
  {
    value: "dark",
    icon: <Moon className="h-5 w-5" />,
    label: "Dark",
    description: "Always use dark theme",
  },
];

export function AppearanceSection() {
  const { theme, resolvedTheme, setTheme } = useTheme();

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-lg font-semibold text-surface-900 dark:text-surface-50">Appearance</h1>
        <p className="mt-1 text-sm text-surface-500">
          Customize how Variance looks on your device.
        </p>
      </div>

      {/* Theme */}
      <section className="space-y-4">
        <h3 className="text-sm font-semibold text-surface-900 dark:text-surface-50">Theme</h3>

        <div className="grid grid-cols-3 gap-3">
          {options.map((opt) => (
            <button
              key={opt.value}
              onClick={() => setTheme(opt.value)}
              className={cn(
                "flex flex-col items-center gap-2 rounded-lg border p-4 transition-colors cursor-pointer",
                theme === opt.value
                  ? "border-primary-500 bg-primary-50 text-primary-600 dark:bg-primary-900/20 dark:text-primary-400"
                  : "border-surface-200 bg-surface-50 text-surface-600 hover:border-surface-300 dark:border-surface-800 dark:bg-surface-900 dark:text-surface-400 dark:hover:border-surface-700"
              )}
            >
              {opt.icon}
              <span className="text-sm font-medium">{opt.label}</span>
              <span className="text-xs text-surface-400">{opt.description}</span>
            </button>
          ))}
        </div>

        <p className="text-sm text-surface-400">
          Currently using{" "}
          <span className="font-medium text-surface-600 dark:text-surface-300">
            {resolvedTheme}
          </span>{" "}
          theme.
        </p>
      </section>
    </div>
  );
}
