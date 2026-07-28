import { useEffect, useId, useRef, useState } from "react";

export interface CompactComboboxOption {
  value: string;
  label: string;
  hint?: string;
}

interface CompactComboboxProps {
  ariaLabel: string;
  value: string;
  options: CompactComboboxOption[];
  disabled?: boolean;
  placeholder?: string;
  onChange: (value: string) => void;
}

export default function CompactCombobox({
  ariaLabel,
  value,
  options,
  disabled = false,
  placeholder = "请选择",
  onChange,
}: CompactComboboxProps) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const listboxId = useId();
  const selected = options.find((option) => option.value === value);

  useEffect(() => {
    if (!open) return undefined;
    const closeOnOutsideClick = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", closeOnOutsideClick);
    const selectedIndex = Math.max(0, options.findIndex((option) => option.value === value));
    setActiveIndex(selectedIndex);
    window.requestAnimationFrame(() => optionRefs.current[selectedIndex]?.focus());
    return () => document.removeEventListener("pointerdown", closeOnOutsideClick);
  }, [open, options, value]);

  const close = () => {
    setOpen(false);
  };

  const focusOption = (index: number) => {
    const nextIndex = (index + options.length) % options.length;
    setActiveIndex(nextIndex);
    optionRefs.current[nextIndex]?.focus();
  };

  return (
    <div className="compact-combobox" ref={rootRef}>
      <button
        className="compact-combobox-trigger"
        type="button"
        role="combobox"
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={listboxId}
        aria-haspopup="listbox"
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            if (open) {
              focusOption(activeIndex + 1);
            } else {
              setOpen(true);
            }
          }
          if (event.key === "ArrowUp") {
            event.preventDefault();
            if (open) {
              focusOption(activeIndex - 1);
            } else {
              setOpen(true);
            }
          }
          if (event.key === "Escape") close();
        }}
      >
        <span className={selected ? "" : "placeholder-copy"}>
          {selected?.label ?? placeholder}
        </span>
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <path d="m4 6 4 4 4-4" />
        </svg>
      </button>

      {open && (
        <div className="compact-combobox-popover">
          <div className="compact-combobox-options" id={listboxId} role="listbox">
            {options.map((option, index) => (
              <button
                key={option.value || "__empty__"}
                className={`compact-combobox-option ${option.value === value ? "selected" : ""}`}
                type="button"
                role="option"
                aria-selected={option.value === value}
                ref={(element) => {
                  optionRefs.current[index] = element;
                }}
                onClick={() => {
                  onChange(option.value);
                  close();
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
                    focusOption(options.length - 1);
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    close();
                  }
                }}
              >
                <span>
                  <strong>{option.label}</strong>
                  {option.hint && <small>{option.hint}</small>}
                </span>
                {option.value === value && (
                  <svg className="compact-combobox-check" viewBox="0 0 18 18" aria-hidden="true">
                    <path d="m3.5 9.25 3.25 3.25 7.75-7.75" />
                  </svg>
                )}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
