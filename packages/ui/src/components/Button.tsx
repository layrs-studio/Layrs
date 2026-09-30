import { forwardRef, useId, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Spinner } from "./Feedback";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ButtonSize = "sm" | "md" | "lg";

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  icon?: ReactNode;
  /** Shows a spinner, replaces the label and blocks double submission. */
  busy?: boolean;
  busyLabel?: string;
  /**
   * Why the action is unavailable. The button stays focusable so keyboard and
   * screen reader users can discover the reason; activation is ignored.
   */
  disabledReason?: string;
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "md", icon, busy, busyLabel, disabledReason, className, children, onClick, disabled, type = "button", ...props },
  ref,
) {
  const reasonId = useId();
  const unavailable = Boolean(disabledReason) && !busy;
  const classes = ["layrs-button", `layrs-button--${variant}`, `layrs-button--${size}`, className].filter(Boolean).join(" ");
  const button = (
    <button
      {...props}
      ref={ref}
      type={type}
      className={classes}
      disabled={disabled || (busy && !unavailable)}
      aria-disabled={unavailable || undefined}
      aria-busy={busy || undefined}
      aria-describedby={unavailable ? reasonId : props["aria-describedby"]}
      onClick={(event) => {
        if (unavailable || busy) {
          event.preventDefault();
          return;
        }
        onClick?.(event);
      }}
    >
      {busy ? <Spinner size={size === "sm" ? 12 : 14} /> : icon}
      {(busy && busyLabel) || children ? <span>{busy && busyLabel ? busyLabel : children}</span> : null}
    </button>
  );
  if (!unavailable) return button;
  return (
    <span className="layrs-reason" data-reason={disabledReason}>
      {button}
      <span id={reasonId} className="layrs-visually-hidden">
        {disabledReason}
      </span>
    </span>
  );
});

export type IconButtonProps = Omit<ButtonProps, "children" | "icon"> & {
  /** Accessible name; also shown as tooltip. */
  label: string;
  icon: ReactNode;
};

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, icon, variant = "ghost", className, ...props },
  ref,
) {
  return (
    <Button
      ref={ref}
      variant={variant}
      icon={icon}
      aria-label={label}
      title={props.disabledReason ?? label}
      className={["layrs-icon-button", className].filter(Boolean).join(" ")}
      {...props}
    />
  );
});
