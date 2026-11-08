/**
 * URLs for the `ibex://` scheme, which streams file bytes straight to the
 * webview with Range support. Media never goes through `invoke`: that would
 * base64 a video into a JSON message.
 */

/**
 * Windows serves custom schemes over `http://<scheme>.localhost`, every other
 * platform over `<scheme>://localhost`. Linux is the primary target, but the
 * helper branches so Windows keeps working.
 */
function origin(): string {
  const isWindows =
    typeof navigator !== "undefined" &&
    /Windows|Win32|Win64/i.test(navigator.userAgent);
  return isWindows ? "http://ibex.localhost" : "ibex://localhost";
}

/** The raw bytes of one file in an open backup. */
export function fileUrl(handle: string, fileId: string): string {
  return `${origin()}/file/${encodeURIComponent(handle)}/${encodeURIComponent(fileId)}`;
}

/**
 * A displayable version of a file: a web-safe image passed through
 * unchanged, or a HEIC/HEIF re-encoded to JPEG. `maxEdge` caps the long edge;
 * omit it for full size (the lightbox).
 */
export function thumbUrl(
  handle: string,
  fileId: string,
  maxEdge?: number,
): string {
  const base = `${origin()}/thumb/${encodeURIComponent(handle)}/${encodeURIComponent(fileId)}`;
  return maxEdge ? `${base}?s=${maxEdge}` : base;
}
