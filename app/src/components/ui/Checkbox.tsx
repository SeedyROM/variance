import { forwardRef, useId } from "react";
import { Check } from "lucide-react";
import { cn } from "../../utils/cn";

interface CheckboxProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "type"> {
  label?: string;
  error?: string;
}

/**
 * Custom styled checkbox.  Hides the native `<input>` and renders a
 * themed box with a check-mark icon.  Forwards ref to the hidden input
 * so form libraries still work.
 *
 * ```tsx
 * <Checkbox
 *   checked={agreed}
 *   onChange={(e) => setAgreed(e.target.checked)}
 *   label="I agree to the terms"
 * />
 * ```
 */
export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(
  ({ className, label, error, id, checked, disabled, ...props }, ref) => {
    const autoId = useId();
    const checkboxId = id ?? autoId;

    return (
      <div className="flex flex-col gap-1">
        <label
          htmlFor={checkboxId}
          className={cn(
            "flex cursor-pointer items-start gap-3 select-none",
            disabled && "cursor-not-allowed opacity-50"
          )}
        >
          {/* Hidden native input for accessibility / form submission */}
          <input
            ref={ref}
            id={checkboxId}
            type="checkbox"
            checked={checked}
            disabled={disabled}
            className="sr-only peer"
            {...props}
          />

          {/* Custom visual */}
          <span
            aria-hidden
            className={cn(
              "mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors",
              "border-surface-300 bg-surface-50",
              "dark:border-surface-600 dark:bg-surface-900",
              checked &&
                "border-primary-500 bg-primary-500 dark:border-primary-500 dark:bg-primary-500",
              !disabled && !checked && "group-hover:border-surface-400",
              error && "border-red-500",
              className
            )}
          >
            {checked && <Check className="h-3 w-3 text-white" strokeWidth={3} />}
          </span>

          {label && <span className="text-sm text-surface-700 dark:text-surface-300">{label}</span>}
        </label>
        {error && <p className="text-xs text-red-500">{error}</p>}
      </div>
    );
  }
);

Checkbox.displayName = "Checkbox";
