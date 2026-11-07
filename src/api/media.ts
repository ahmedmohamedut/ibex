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
