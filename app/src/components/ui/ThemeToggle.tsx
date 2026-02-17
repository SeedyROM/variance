import { Moon, Sun, Monitor } from "lucide-react";
import { useTheme, type Theme } from "../../hooks/useTheme";
import { cn } from "../../utils/cn";

export function ThemeToggle() {
  const { theme, setTheme } = useTheme();

  const options: { value: Theme; icon: React.ReactNode; label: string }[] = [
    { value: "light", icon: <Sun className="h-4 w-4" />, label: "Light" },
    { value: "system", icon: <Monitor className="h-4 w-4" />, label: "System" },
    { value: "dark", icon: <Moon className="h-4 w-4" />, label: "Dark" },
  ];

  return (
    <div className="flex items-center gap-1 rounded-lg bg-surface-200 p-1 dark:bg-surface-800">
      {options.map((opt) => (
        <button
          key={opt.value}
          onClick={() => setTheme(opt.value)}
          title={opt.label}
          className={cn(
            "rounded-md p-1.5 transition-colors",
            theme === opt.value
              ? "bg-surface-50 text-primary-500 shadow-sm dark:bg-surface-900"
              : "text-surface-500 hover:text-surface-700 dark:hover:text-surface-300"
          )}
        >
          {opt.icon}
        </button>
      ))}
    </div>
  );
}
