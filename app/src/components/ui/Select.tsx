import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { ChevronDown } from "lucide-react";
import { cn } from "../../utils/cn";

/* ------------------------------------------------------------------ */
/*  Option                                                             */
/* ------------------------------------------------------------------ */

interface OptionProps {
  value: string | number;
  children: ReactNode;
  disabled?: boolean;
}

interface OptionEntry {
  value: string | number;
  label: ReactNode;
  disabled?: boolean;
}

interface SelectCtx {
  selected: string | number | undefined;
  highlighted: number;
  onSelect: (value: string | number) => void;
  onHighlight: (index: number) => void;
  registerOption: (entry: OptionEntry) => number;
}

const SelectContext = createContext<SelectCtx | null>(null);

/**
 * A single option inside a `<Select>`.
 *
 * ```tsx
 * <Select value={v} onChange={setV}>
 *   <Option value="a">Alpha</Option>
 *   <Option value="b">Beta</Option>
 * </Select>
 * ```
 */
export function Option({ value, children, disabled }: OptionProps) {
  const ctx = useContext(SelectContext);
  const indexRef = useRef(-1);

  // Register on mount so Select knows about us.
  useEffect(() => {
    if (ctx) {
      indexRef.current = ctx.registerOption({ value, label: children, disabled });
    }
    // Only register once on mount — the parent rebuilds the list each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!ctx) return null;

  const isSelected = ctx.selected !== undefined && String(ctx.selected) === String(value);
  const isHighlighted = ctx.highlighted === indexRef.current;

  return (
    <div
      role="option"
      aria-selected={isSelected}
      aria-disabled={disabled}
      data-index={indexRef.current}
      onMouseEnter={() => {
        if (!disabled) ctx.onHighlight(indexRef.current);
      }}
      onMouseDown={(e) => {
        // Prevent blur on the trigger so the menu stays manageable.
        e.preventDefault();
        if (!disabled) ctx.onSelect(value);
      }}
      className={cn(
        "flex items-center px-3 py-1.5 text-sm cursor-pointer select-none transition-colors",
        isHighlighted && "bg-surface-200/60 dark:bg-surface-700/60",
        isSelected && "text-primary-500 font-medium",
        !isSelected && "text-surface-800 dark:text-surface-200",
        disabled && "text-surface-400 dark:text-surface-500 cursor-default opacity-50"
      )}
    >
      {children}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Select                                                             */
/* ------------------------------------------------------------------ */

interface SelectProps {
  value?: string | number;
  onChange?: (value: string | number) => void;
  children: ReactNode;
  label?: string;
  error?: string;
  id?: string;
  disabled?: boolean;
  className?: string;
  placeholder?: string;
}

/**
 * Custom styled dropdown that replaces native `<select>`.
 * Uses `<Option>` children for items and renders a portal-based listbox.
 *
 * ```tsx
 * <Select value={val} onChange={setVal} label="Theme">
 *   <Option value="light">Light</Option>
 *   <Option value="dark">Dark</Option>
 * </Select>
 * ```
 */
export function Select({
  value,
  onChange,
  children,
  label,
  error,
  id,
  disabled,
  className,
  placeholder = "Select\u2026",
}: SelectProps) {
  const [open, setOpen] = useState(false);
  const [highlighted, setHighlighted] = useState(-1);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const optionsRef = useRef<OptionEntry[]>([]);
  const selectId = id ?? label?.toLowerCase().replace(/\s+/g, "-");

  // Rebuild the options list every render (children may change).
  optionsRef.current = [];
  let registerIndex = 0;
  const registerOption = useCallback((entry: OptionEntry): number => {
    const idx = registerIndex;
    optionsRef.current.push(entry);
    registerIndex++;
    return idx;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, value, children]);

  const selectedOption = optionsRef.current.find(
    (o) => o.value !== undefined && String(o.value) === String(value)
  );

  function handleSelect(v: string | number) {
    onChange?.(v);
    setOpen(false);
    triggerRef.current?.focus();
  }

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (
        menuRef.current &&
        !menuRef.current.contains(e.target as Node) &&
        triggerRef.current &&
        !triggerRef.current.contains(e.target as Node)
      ) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown, true);
    return () => document.removeEventListener("mousedown", onDown, true);
  }, [open]);

  // Keyboard navigation
  function handleKeyDown(e: React.KeyboardEvent) {
    const opts = optionsRef.current;
    if (e.key === "Escape") {
      setOpen(false);
      return;
    }
    if (!open && (e.key === "Enter" || e.key === " " || e.key === "ArrowDown")) {
      e.preventDefault();
      setOpen(true);
      // Highlight the currently selected option, or the first one.
      const selIdx = opts.findIndex((o) => String(o.value) === String(value));
      setHighlighted(selIdx >= 0 ? selIdx : 0);
      return;
    }
    if (!open) return;

    if (e.key === "ArrowDown") {
      e.preventDefault();
      let next = highlighted + 1;
      while (next < opts.length && opts[next].disabled) next++;
      if (next < opts.length) setHighlighted(next);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      let prev = highlighted - 1;
      while (prev >= 0 && opts[prev].disabled) prev--;
      if (prev >= 0) setHighlighted(prev);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (highlighted >= 0 && highlighted < opts.length && !opts[highlighted].disabled) {
        handleSelect(opts[highlighted].value);
      }
    }
  }

  // Position the dropdown relative to the trigger
  const [menuStyle, setMenuStyle] = useState<React.CSSProperties>({});
  useEffect(() => {
    if (!open || !triggerRef.current) return;
    const rect = triggerRef.current.getBoundingClientRect();
    setMenuStyle({
      position: "fixed",
      top: rect.bottom + 4,
      left: rect.left,
      minWidth: rect.width,
    });
  }, [open]);

  return (
    <div className="flex flex-col gap-1">
      {label && (
        <label
          htmlFor={selectId}
          className="text-sm font-medium text-surface-700 dark:text-surface-300"
        >
          {label}
        </label>
      )}

      <button
        ref={triggerRef}
        id={selectId}
        type="button"
        role="combobox"
        aria-expanded={open}
        aria-haspopup="listbox"
        disabled={disabled}
        onClick={() => {
          if (!disabled) {
            setOpen((o) => !o);
            if (!open) {
              const selIdx = optionsRef.current.findIndex(
                (o) => String(o.value) === String(value)
              );
              setHighlighted(selIdx >= 0 ? selIdx : 0);
            }
          }
        }}
        onKeyDown={handleKeyDown}
        className={cn(
          "inline-flex items-center justify-between gap-2 rounded-lg border px-3 py-2 text-sm text-left transition-colors",
          "border-surface-300 bg-surface-50 text-surface-900",
          "hover:border-surface-400",
          "focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/20",
          "dark:border-surface-800 dark:bg-surface-900 dark:text-surface-50 dark:hover:border-surface-600",
          "disabled:cursor-not-allowed disabled:opacity-50",
          error && "border-red-500 focus:border-red-500 focus:ring-red-500/20",
          className
        )}
      >
        <span className={cn("truncate", !selectedOption && "text-surface-400")}>
          {selectedOption ? selectedOption.label : placeholder}
        </span>
        <ChevronDown
          className={cn(
            "h-3.5 w-3.5 shrink-0 text-surface-400 transition-transform",
            open && "rotate-180"
          )}
        />
      </button>

      {error && <p className="text-xs text-red-500">{error}</p>}

      {/* Render options children so they register, but visually hidden when closed */}
      <SelectContext.Provider
        value={{
          selected: value,
          highlighted,
          onSelect: handleSelect,
          onHighlight: setHighlighted,
          registerOption,
        }}
      >
        {open &&
          createPortal(
            <div
              ref={menuRef}
              role="listbox"
              style={{ ...menuStyle, animation: "tooltip-in 0.1s ease-out" }}
              className={cn(
                "z-50 rounded-lg py-1 overflow-auto max-h-60",
                "bg-surface-50 dark:bg-surface-800",
                "border border-surface-200 dark:border-surface-700",
                "shadow-xl shadow-black/20"
              )}
            >
              {children}
            </div>,
            document.body
          )}

        {/* Hidden render so options register even when closed (for selectedOption label) */}
        {!open && <div className="hidden">{children}</div>}
      </SelectContext.Provider>
    </div>
  );
}
