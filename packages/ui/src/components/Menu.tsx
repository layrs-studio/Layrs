import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

export type MenuItem = {
  id: string;
  label: ReactNode;
  icon?: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  /** Destructive actions render last, separated and in the danger tone. */
  danger?: boolean;
};

/** Secondary actions behind a trigger button. */
export function Menu({
  label,
  trigger,
  items,
  align = "end",
}: {
  /** Accessible name of the trigger. */
  label: string;
  trigger: ReactNode;
  items: MenuItem[];
  align?: "start" | "end";
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const menuId = useId();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const ordered = [...items.filter((item) => !item.danger), ...items.filter((item) => item.danger)];
  const firstDanger = ordered.findIndex((item) => item.danger);

  useEffect(() => {
    if (!open) return;
    function onPointerDown(event: PointerEvent) {
      const target = event.target as Node;
      if (!listRef.current?.contains(target) && !triggerRef.current?.contains(target)) setOpen(false);
    }
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  useEffect(() => {
    if (open) listRef.current?.querySelectorAll<HTMLButtonElement>("[role=menuitem]")[active]?.focus();
  }, [open, active]);

  function close(returnFocus = true) {
    setOpen(false);
    if (returnFocus) triggerRef.current?.focus();
  }

  function openAt(index: number) {
    setActive(index);
    setOpen(true);
  }

  function onListKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "ArrowDown") setActive((index) => (index + 1) % ordered.length);
    else if (event.key === "ArrowUp") setActive((index) => (index - 1 + ordered.length) % ordered.length);
    else if (event.key === "Home") setActive(0);
    else if (event.key === "End") setActive(ordered.length - 1);
    else if (event.key === "Escape") close();
    else if (event.key === "Tab") close(false);
    else return;
    if (event.key !== "Tab") event.preventDefault();
  }

  return (
    <div className={`layrs-menu layrs-menu--${align}`}>
      <button
        ref={triggerRef}
        type="button"
        className="layrs-menu__trigger"
        aria-label={label}
        title={label}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => (open ? close() : openAt(0))}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            openAt(0);
          } else if (event.key === "ArrowUp") {
            event.preventDefault();
            openAt(ordered.length - 1);
          }
        }}
      >
        {trigger}
      </button>
      {open && (
        <div ref={listRef} id={menuId} role="menu" aria-label={label} className="layrs-menu__list" onKeyDown={onListKeyDown}>
          {ordered.map((item, index) => (
            <div key={item.id}>
              {index === firstDanger && index > 0 && <div role="separator" className="layrs-menu__separator" />}
              <button
                type="button"
                role="menuitem"
                tabIndex={index === active ? 0 : -1}
                disabled={item.disabled}
                className={["layrs-menu__item", item.danger && "layrs-menu__item--danger"].filter(Boolean).join(" ")}
                onClick={() => {
                  close();
                  item.onSelect();
                }}
              >
                {item.icon}
                <span>{item.label}</span>
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
