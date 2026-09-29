import { useRef, type KeyboardEvent, type ReactNode } from "react";

export type SegmentedOption<T extends string> = { value: T; label: ReactNode; icon?: ReactNode };

/** Mutually exclusive choice applied immediately (radio group semantics). */
export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  label,
  size = "md",
}: {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  label: string;
  size?: "sm" | "md";
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const index = options.findIndex((option) => option.value === value);
    let next = index;
    if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (index + 1) % options.length;
    else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (index - 1 + options.length) % options.length;
    else return;
    event.preventDefault();
    onChange(options[next].value);
    refs.current[next]?.focus();
  }
  return (
    <div className={`layrs-segmented layrs-segmented--${size}`} role="radiogroup" aria-label={label} onKeyDown={onKeyDown}>
      {options.map((option, index) => {
        const checked = option.value === value;
        return (
          <button
            key={option.value}
            ref={(element) => {
              refs.current[index] = element;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={checked ? 0 : -1}
            className="layrs-segmented__option"
            onClick={() => onChange(option.value)}
          >
            {option.icon}
            <span>{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}
