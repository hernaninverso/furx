import { ReactNode } from "react";

export type PillTone = "default" | "warning" | "danger" | "success";

interface PillBase {
  label?: string;
  value: ReactNode;
  tone?: PillTone;
  stale?: boolean;
  title?: string;
  ariaLabel?: string;
}

interface PillStatus extends PillBase {
  onClick?: undefined;
  active?: undefined;
  ariaPressed?: undefined;
}

interface PillButton extends PillBase {
  onClick: () => void;
  active?: boolean;
  ariaPressed?: boolean;
}

export type PillProps = PillStatus | PillButton;

function classes(props: PillProps): string {
  const cls = ["pill"];
  if (props.onClick) cls.push("pill--button");
  if ("active" in props && props.active) cls.push("pill--active");
  if (props.stale) cls.push("pill--stale");
  if (props.tone && props.tone !== "default") cls.push(`pill--${props.tone}`);
  return cls.join(" ");
}

export function Pill(props: PillProps) {
  const cls = classes(props);
  const content = (
    <>
      {props.label && <span className="pill-label">{props.label}</span>}
      <span className="pill-value">{props.value}</span>
    </>
  );
  if (props.onClick) {
    return (
      <button
        type="button"
        className={cls}
        onClick={props.onClick}
        title={props.title}
        aria-label={props.ariaLabel}
        aria-pressed={typeof props.ariaPressed === "boolean" ? props.ariaPressed : undefined}
      >
        {content}
      </button>
    );
  }
  return (
    <span className={cls} title={props.title} aria-label={props.ariaLabel}>
      {content}
    </span>
  );
}
