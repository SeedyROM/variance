import { forwardRef } from "react";
import { cn } from "../../utils/cn";

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  error?: string;
  /** Allow OS typing suggestions (autocorrect, autocapitalize, spellcheck). Default: false. */
  allowSuggestions?: boolean;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ className, label, error, id, allowSuggestions = false, ...props }, ref) => {
    const inputId = id ?? label?.toLowerCase().replace(/\s+/g, "-");

    return (
      <div className="flex flex-col gap-1">
        {label && (
          <label
            htmlFor={inputId}
            className="text-sm font-medium text-surface-700 dark:text-surface-300"
          >
            {label}
          </label>
        )}
        <input
          ref={ref}
          id={inputId}
          autoCorrect={allowSuggestions ? "on" : "off"}
          autoCapitalize={allowSuggestions ? "sentences" : "none"}
          spellCheck={allowSuggestions}
          className={cn(
            "w-full rounded-lg border border-surface-300 bg-surface-50 px-3 py-2 text-sm text-surface-900 placeholder:text-surface-400",
            "focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/20",
            "dark:border-surface-800 dark:bg-surface-900 dark:text-surface-50 dark:placeholder:text-surface-600",
            "disabled:cursor-not-allowed disabled:opacity-50",
            error && "border-red-500 focus:border-red-500 focus:ring-red-500/20",
            className
          )}
          {...props}
        />
        {error && <p className="text-xs text-red-500">{error}</p>}
      </div>
    );
  }
);

Input.displayName = "Input";
