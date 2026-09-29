import { useId, useRef, type KeyboardEvent, type ReactNode } from "react";

export type TabItem<T extends string> = { id: T; label: ReactNode; count?: number };

/**
 * Sub-views of one object. Arrow keys move between tabs (roving tabindex).
 * `children` is the content of the selected tab.
 */
export function Tabs<T extends string>({
  items,
  value,
  onChange,
  label,
  children,
}: {
  items: TabItem<T>[];
  value: T;
  onChange: (value: T) => void;
  label: string;
  children?: ReactNode;
}) {
  const baseId = useId();
  const refs = useRef(new Map<T, HTMLButtonElement>());

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const index = items.findIndex((item) => item.id === value);
    let next = index;
    if (event.key === "ArrowRight") next = (index + 1) % items.length;
    else if (event.key === "ArrowLeft") next = (index - 1 + items.length) % items.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = items.length - 1;
    else return;
    event.preventDefault();
    const target = items[next];
    onChange(target.id);
    refs.current.get(target.id)?.focus();
  }

  return (
    <div className="layrs-tabs">
      <div className="layrs-tabs__list" role="tablist" aria-label={label} onKeyDown={onKeyDown}>
        {items.map((item) => {
          const selected = item.id === value;
          return (
            <button
              key={item.id}
              ref={(element) => {
                if (element) refs.current.set(item.id, element);
                else refs.current.delete(item.id);
              }}
              type="button"
              role="tab"
              id={`${baseId}-tab-${item.id}`}
              aria-selected={selected}
              aria-controls={`${baseId}-panel`}
              tabIndex={selected ? 0 : -1}
              className="layrs-tab"
              onClick={() => onChange(item.id)}
            >
              {item.label}
              {item.count !== undefined && <span className="layrs-tab__count">{item.count}</span>}
            </button>
          );
        })}
      </div>
      {children !== undefined && (
        <div role="tabpanel" id={`${baseId}-panel`} aria-labelledby={`${baseId}-tab-${value}`} className="layrs-tabs__panel">
          {children}
        </div>
      )}
    </div>
  );
}
