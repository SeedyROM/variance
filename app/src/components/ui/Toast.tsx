import { X } from "lucide-react";
import { cn } from "../../utils/cn";
import { useToastStore, type Toast } from "../../stores/toastStore";

export function ToastItem({ toast }: { toast: Toast }) {
  const removeToast = useToastStore((s) => s.removeToast);
  return (
    <div
      className={cn(
        "flex items-start gap-3 rounded-lg px-4 py-3 shadow-lg text-sm font-medium text-white",
        {
          "bg-red-600": toast.variant === "error",
          "bg-green-600": toast.variant === "success",
          "bg-surface-700 dark:bg-surface-600": toast.variant === "info",
        }
      )}
    >
      <span className="flex-1">{toast.message}</span>
      <button
        type="button"
        onClick={() => removeToast(toast.id)}
        className="shrink-0 opacity-80 hover:opacity-100 transition-opacity"
        aria-label="Dismiss"
      >
        <X className="h-4 w-4" />
      </button>
    </div>
  );
}
