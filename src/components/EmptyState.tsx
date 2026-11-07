import type { ReactNode } from "react";

/**
 * An empty or error state. Per UI_DESIGN.md these always say *why* and what to
 * do next, never just "nothing here".
 */
export function EmptyState(props: {
  title: string;
  detail?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty__title">{props.title}</div>
      {props.detail ? <div>{props.detail}</div> : null}
      {props.action}
    </div>
  );
}

export function Spinner(props: { label?: string }) {
  return (
    <div className="empty">
      <div className="spinner" role="status" aria-label={props.label ?? "Loading"} />
      {props.label ? <div>{props.label}</div> : null}
    </div>
  );
}
