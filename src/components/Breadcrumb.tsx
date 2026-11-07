/** Clickable path segments for the Raw browser. */
export function Breadcrumb(props: {
  root: string;
  path: string;
  onNavigate: (dir: string) => void;
}) {
  const segments = props.path.split("/").filter(Boolean);

  return (
    <nav className="crumbs" aria-label="Path">
      <button
        type="button"
        className="crumbs__item"
        onClick={() => props.onNavigate("")}
        disabled={segments.length === 0}
      >
        {props.root}
      </button>
      {segments.map((segment, index) => {
        const target = segments.slice(0, index + 1).join("/");
        const isLast = index === segments.length - 1;
        return (
          <span key={target} className="crumbs">
            <span className="crumbs__sep" aria-hidden="true">
              /
            </span>
            <button
              type="button"
              className="crumbs__item"
              onClick={() => props.onNavigate(target)}
              disabled={isLast}
            >
              {segment}
            </button>
          </span>
        );
      })}
    </nav>
  );
}
