import {
  forwardRef,
  useId,
  useState,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes,
} from "react";
import { Eye, EyeOff } from "lucide-react";

type FieldFrameProps = {
  label: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  /** Visually hides the label while keeping it for assistive technology. */
  hideLabel?: boolean;
  className?: string;
};

function describedBy(ids: unknown[]) {
  const value = ids.filter((id): id is string => typeof id === "string" && id.length > 0).join(" ");
  return value || undefined;
}

function FieldFrame({
  id,
  label,
  hint,
  error,
  hideLabel,
  className,
  children,
}: FieldFrameProps & { id: string; children: ReactNode }) {
  return (
    <div className={["layrs-field", error && "layrs-field--invalid", className].filter(Boolean).join(" ")}>
      <label htmlFor={id} className={hideLabel ? "layrs-visually-hidden" : "layrs-field__label"}>
        {label}
      </label>
      {children}
      {hint && !error && (
        <p id={`${id}-hint`} className="layrs-field__hint">
          {hint}
        </p>
      )}
      {error && (
        <p id={`${id}-error`} className="layrs-field__error">
          {error}
        </p>
      )}
    </div>
  );
}

export type TextFieldProps = FieldFrameProps & Omit<InputHTMLAttributes<HTMLInputElement>, "className"> & { trailing?: ReactNode };

export const TextField = forwardRef<HTMLInputElement, TextFieldProps>(function TextField(
  { label, hint, error, hideLabel, className, trailing, id, ...props },
  ref,
) {
  const generated = useId();
  const fieldId = id ?? generated;
  return (
    <FieldFrame id={fieldId} label={label} hint={hint} error={error} hideLabel={hideLabel} className={className}>
      <div className="layrs-input">
        <input
          ref={ref}
          id={fieldId}
          aria-invalid={error ? true : undefined}
          aria-describedby={describedBy([hint && !error && `${fieldId}-hint`, error && `${fieldId}-error`])}
          {...props}
        />
        {trailing}
      </div>
    </FieldFrame>
  );
});

export const PasswordField = forwardRef<HTMLInputElement, Omit<TextFieldProps, "type" | "trailing">>(function PasswordField(props, ref) {
  const [revealed, setRevealed] = useState(false);
  return (
    <TextField
      ref={ref}
      {...props}
      type={revealed ? "text" : "password"}
      trailing={
        <button
          type="button"
          className="layrs-input__action"
          aria-pressed={revealed}
          aria-label={revealed ? "Masquer le mot de passe" : "Afficher le mot de passe"}
          title={revealed ? "Masquer le mot de passe" : "Afficher le mot de passe"}
          onClick={() => setRevealed((value) => !value)}
        >
          {revealed ? <EyeOff aria-hidden size={16} /> : <Eye aria-hidden size={16} />}
        </button>
      }
    />
  );
});

export type TextAreaFieldProps = FieldFrameProps & Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, "className">;

export const TextAreaField = forwardRef<HTMLTextAreaElement, TextAreaFieldProps>(function TextAreaField(
  { label, hint, error, hideLabel, className, id, ...props },
  ref,
) {
  const generated = useId();
  const fieldId = id ?? generated;
  return (
    <FieldFrame id={fieldId} label={label} hint={hint} error={error} hideLabel={hideLabel} className={className}>
      <textarea
        ref={ref}
        id={fieldId}
        className="layrs-textarea"
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy([hint && !error && `${fieldId}-hint`, error && `${fieldId}-error`])}
        {...props}
      />
    </FieldFrame>
  );
});

export type SelectFieldProps = FieldFrameProps & Omit<SelectHTMLAttributes<HTMLSelectElement>, "className">;

export const SelectField = forwardRef<HTMLSelectElement, SelectFieldProps>(function SelectField(
  { label, hint, error, hideLabel, className, id, children, ...props },
  ref,
) {
  const generated = useId();
  const fieldId = id ?? generated;
  return (
    <FieldFrame id={fieldId} label={label} hint={hint} error={error} hideLabel={hideLabel} className={className}>
      <select
        ref={ref}
        id={fieldId}
        className="layrs-select"
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy([hint && !error && `${fieldId}-hint`, error && `${fieldId}-error`])}
        {...props}
      >
        {children}
      </select>
    </FieldFrame>
  );
});

export function Checkbox({
  label,
  hint,
  className,
  ...props
}: Omit<InputHTMLAttributes<HTMLInputElement>, "type"> & { label: ReactNode; hint?: ReactNode }) {
  const id = useId();
  return (
    <div className={["layrs-check", className].filter(Boolean).join(" ")}>
      <input id={id} type="checkbox" aria-describedby={hint ? `${id}-hint` : undefined} {...props} />
      <div>
        <label htmlFor={id}>{label}</label>
        {hint && (
          <p id={`${id}-hint`} className="layrs-field__hint">
            {hint}
          </p>
        )}
      </div>
    </div>
  );
}

/** On/off setting that applies immediately. */
export function Switch({
  label,
  hint,
  checked,
  onChange,
  disabled,
  hideLabel,
}: {
  label: ReactNode;
  hint?: ReactNode;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  /** Keeps the label for assistive technology when a visible status already names the setting. */
  hideLabel?: boolean;
}) {
  const id = useId();
  return (
    <div className="layrs-switch">
      <button
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        aria-describedby={hint ? `${id}-hint` : undefined}
        disabled={disabled}
        className="layrs-switch__control"
        onClick={() => onChange(!checked)}
      >
        <span className="layrs-switch__thumb" aria-hidden />
      </button>
      <div>
        <label htmlFor={id} className={hideLabel ? "layrs-visually-hidden" : undefined}>
          {label}
        </label>
        {hint && (
          <p id={`${id}-hint`} className="layrs-field__hint">
            {hint}
          </p>
        )}
      </div>
    </div>
  );
}
