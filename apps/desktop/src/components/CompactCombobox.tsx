import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { useLocalizedCopy } from "./LanguageProvider";

export interface CompactComboboxOption {
  value: string;
  label: string;
  hint?: string;
  icon?: ReactNode;
}

interface CompactComboboxProps {
  ariaLabel: string;
  ariaDescribedBy?: string;
  ariaInvalid?: boolean;
  value: string;
  options: CompactComboboxOption[];
  disabled?: boolean;
  placeholder?: string;
  onChange: (value: string) => void;
}

const COMBOBOX_OPEN_EVENT = "token-station:combobox-open";
const SEARCH_THRESHOLD = 10;
const INITIAL_OPTION_LIMIT = 100;

export default function CompactCombobox({
  ariaLabel,
  ariaDescribedBy,
  ariaInvalid = false,
  value,
  options,
  disabled = false,
  placeholder,
  onChange,
}: CompactComboboxProps) {
  const { copy } = useLocalizedCopy();
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [query, setQuery] = useState("");
  const [popoverStyle, setPopoverStyle] = useState<CSSProperties>();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const listboxId = useId();
  const instanceId = useId();
  const selected = options.find((option) => option.value === value);
  const searchable = options.length > SEARCH_THRESHOLD;
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const defaultOptions = useMemo(() => {
    if (!searchable || options.length <= INITIAL_OPTION_LIMIT) return options;
    const initial = options.slice(0, INITIAL_OPTION_LIMIT);
    return selected && !initial.some((option) => option.value === selected.value)
      ? [selected, ...initial]
      : initial;
  }, [options, searchable, selected]);
  const visibleOptions = useMemo(() => {
    if (normalizedQuery) {
      return options.filter((option) =>
        `${option.label} ${option.hint ?? ""}`.toLocaleLowerCase().includes(normalizedQuery));
    }
    return defaultOptions;
  }, [defaultOptions, normalizedQuery, options]);

  useEffect(() => {
    if (!open) return undefined;
    const closeOnOutsideClick = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOtherCombobox = (event: Event) => {
      if ((event as CustomEvent<string>).detail !== instanceId) setOpen(false);
    };
    document.addEventListener("pointerdown", closeOnOutsideClick);
    document.addEventListener(COMBOBOX_OPEN_EVENT, closeOtherCombobox);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsideClick);
      document.removeEventListener(COMBOBOX_OPEN_EVENT, closeOtherCombobox);
    };
  }, [instanceId, open]);

  useEffect(() => {
    if (disabled && open) setOpen(false);
  }, [disabled, open]);

  useLayoutEffect(() => {
    if (!open) return undefined;
    const viewportPadding = 12;
    const gap = 6;
    const minimumMenuHeight = 120;
    const maximumMenuHeight = 360;
    const trigger = triggerRef.current;
    const initialRect = trigger?.getBoundingClientRect();
    if (trigger && initialRect) {
      const initialSpaceBelow = window.innerHeight - initialRect.bottom - viewportPadding - gap;
      if (initialSpaceBelow < minimumMenuHeight) {
        const previousScrollMarginBottom = trigger.style.scrollMarginBottom;
        trigger.style.scrollMarginBottom =
          `${minimumMenuHeight + viewportPadding + gap}px`;
        trigger.scrollIntoView({ block: "nearest", inline: "nearest" });
        trigger.style.scrollMarginBottom = previousScrollMarginBottom;
      }
    }

    const positionPopover = () => {
      const rect = triggerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const spaceBelow = window.innerHeight - rect.bottom - viewportPadding - gap;
      const maxHeight = Math.max(0, Math.min(maximumMenuHeight, spaceBelow));
      setPopoverStyle({
        left: Math.max(viewportPadding, rect.left),
        top: rect.bottom + gap,
        bottom: "auto",
        width: rect.width,
        maxHeight,
      });
    };
    positionPopover();
    window.addEventListener("resize", positionPopover);
    window.addEventListener("scroll", positionPopover, true);
    return () => {
      window.removeEventListener("resize", positionPopover);
      window.removeEventListener("scroll", positionPopover, true);
    };
  }, [open]);

  useLayoutEffect(() => {
    if (open) optionRefs.current[activeIndex]?.focus();
    // Only the closed → open transition owns initial focus. Arrow-key focus
    // is synchronous and must never be overwritten by a later render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const close = (restoreFocus = false) => {
    setOpen(false);
    setQuery("");
    if (restoreFocus) triggerRef.current?.focus();
  };

  const openMenu = (direction: "selected" | "first" | "last" = "selected") => {
    if (disabled || options.length === 0) return;
    const selectedIndex = Math.max(0, defaultOptions.findIndex((option) => option.value === value));
    const nextIndex = direction === "first"
      ? 0
      : direction === "last"
        ? defaultOptions.length - 1
        : selectedIndex;
    setQuery("");
    setActiveIndex(nextIndex);
    setOpen(true);
    document.dispatchEvent(new CustomEvent(COMBOBOX_OPEN_EVENT, { detail: instanceId }));
  };

  const focusOption = (index: number) => {
    if (visibleOptions.length === 0) return;
    const nextIndex = (index + visibleOptions.length) % visibleOptions.length;
    setActiveIndex(nextIndex);
    optionRefs.current[nextIndex]?.focus();
  };

  return (
    <div
      className="compact-combobox"
      ref={rootRef}
      onBlurCapture={(event) => {
        if (open && !event.currentTarget.contains(event.relatedTarget as Node | null)) close();
      }}
    >
      <button
        ref={triggerRef}
        className="compact-combobox-trigger"
        type="button"
        role="combobox"
        aria-label={ariaLabel}
        aria-describedby={ariaDescribedBy}
        aria-expanded={open}
        aria-controls={listboxId}
        aria-haspopup="listbox"
        aria-invalid={ariaInvalid || undefined}
        disabled={disabled}
        title={selected?.label}
        onClick={() => {
          if (open) close();
          else openMenu();
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            if (open) {
              focusOption(activeIndex + 1);
            } else {
              openMenu("first");
            }
          }
          if (event.key === "ArrowUp") {
            event.preventDefault();
            if (open) {
              focusOption(activeIndex - 1);
            } else {
              openMenu("last");
            }
          }
          if (event.key === "Escape") close(true);
        }}
      >
        <span className={`compact-combobox-value ${selected ? "" : "placeholder-copy"}`}>
          {selected?.icon && <span className="compact-combobox-icon" aria-hidden="true">{selected.icon}</span>}
          <span>{selected?.label ?? placeholder ?? copy("Select", "请选择")}</span>
        </span>
        <svg className="compact-combobox-chevron" viewBox="0 0 16 16" aria-hidden="true">
          <path d="m4 6 4 4 4-4" />
        </svg>
      </button>

      {open && (
        <div
          className="compact-combobox-popover"
          data-onboarding-floating="true"
          style={popoverStyle}
        >
          {searchable && (
            <label className="compact-combobox-search">
              <span aria-hidden="true">⌕</span>
              <input
                ref={searchRef}
                type="search"
                value={query}
                aria-label={copy(`Search ${ariaLabel}`, `搜索${ariaLabel}`)}
                placeholder={copy("Search", "搜索")}
                onChange={(event) => {
                  setQuery(event.target.value);
                  setActiveIndex(0);
                }}
                onKeyDown={(event) => {
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    focusOption(0);
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    close(true);
                  }
                }}
              />
            </label>
          )}
          <div className="compact-combobox-options" id={listboxId} role="listbox">
            {visibleOptions.map((option, index) => (
              <button
                key={option.value || "__empty__"}
                className={`compact-combobox-option ${option.value === value ? "selected" : ""}`}
                type="button"
                role="option"
                aria-selected={option.value === value}
                tabIndex={index === activeIndex ? 0 : -1}
                title={option.label}
                ref={(element) => {
                  optionRefs.current[index] = element;
                }}
                onClick={() => {
                  if (option.value !== value) onChange(option.value);
                  close(true);
                }}
                onFocus={() => setActiveIndex(index)}
                onKeyDown={(event) => {
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    focusOption(activeIndex + 1);
                  } else if (event.key === "ArrowUp") {
                    event.preventDefault();
                    focusOption(activeIndex - 1);
                  } else if (event.key === "Home") {
                    event.preventDefault();
                    focusOption(0);
                  } else if (event.key === "End") {
                    event.preventDefault();
                    focusOption(visibleOptions.length - 1);
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    close(true);
                  } else if (
                    searchable
                    && event.key.length === 1
                    && !event.altKey
                    && !event.ctrlKey
                    && !event.metaKey
                  ) {
                    setQuery(event.key);
                    window.requestAnimationFrame(() => searchRef.current?.focus());
                  }
                }}
              >
                <span className="compact-combobox-option-content">
                  {option.icon && <span className="compact-combobox-icon" aria-hidden="true">{option.icon}</span>}
                  <span>
                    <strong>{option.label}</strong>
                    {option.hint && <small>{option.hint}</small>}
                  </span>
                </span>
                {option.value === value && (
                  <svg className="compact-combobox-check" viewBox="0 0 18 18" aria-hidden="true">
                    <path d="m3.5 9.25 3.25 3.25 7.75-7.75" />
                  </svg>
                )}
              </button>
            ))}
            {visibleOptions.length === 0 && (
              <div className="compact-combobox-empty">
                {copy("No matching options", "没有匹配项")}
              </div>
            )}
            {!normalizedQuery && options.length > INITIAL_OPTION_LIMIT && (
              <div className="compact-combobox-limit">
                {copy("Enter a name to search the remaining options", "输入名称可搜索其余选项")}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
