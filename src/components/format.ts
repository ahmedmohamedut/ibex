/** Shared formatting helpers. */

/** Bytes as a human-readable size, binary units. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let size = bytes / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(1)} ${units[unit]}`;
}

/** An RFC 3339 string as a local date and time, or an em dash. */
export function formatDateTime(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** An RFC 3339 string as a local date only. */
export function formatDate(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "2-digit",
  });
}

/** A count with thousands separators. */
export function formatCount(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return value.toLocaleString();
}

/** Unix mode bits as `rwxr-xr-x`. */
export function formatMode(mode: number | null | undefined): string {
  if (mode === null || mode === undefined) return "—";
  const bits = mode & 0o777;
  const rwx = (triple: number) =>
    `${triple & 4 ? "r" : "-"}${triple & 2 ? "w" : "-"}${triple & 1 ? "x" : "-"}`;
  return `${rwx((bits >> 6) & 7)}${rwx((bits >> 3) & 7)}${rwx(bits & 7)} (${bits.toString(8)})`;
}
