import { invoke } from "@tauri-apps/api/core";
import type { IpcError } from "../bindings/IpcError";

/**
 * An error raised by a Rust command.
 *
 * Commands reject with the `IpcError` shape rather than a string, so the UI can
 * branch on `code` (a wrong password needs its own state, not a toast).
 */
export class CommandError extends Error {
  readonly code: string;

  constructor(error: IpcError) {
    super(error.message);
    this.name = "CommandError";
    this.code = error.code;
  }
}

/** True when the value looks like the `IpcError` our commands return. */
function isIpcError(value: unknown): value is IpcError {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as IpcError).code === "string" &&
    typeof (value as IpcError).message === "string"
  );
}

/** Call a Rust command, normalising whatever it throws into a CommandError. */
export async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (raw) {
    if (isIpcError(raw)) throw new CommandError(raw);
    // A panic or a serialization failure arrives as a bare string.
    throw new CommandError({
      code: "unknown",
      message: typeof raw === "string" ? raw : String(raw),
    });
  }
}

/** The code of an error, or undefined if it isn't one of ours. */
export function errorCode(error: unknown): string | undefined {
  return error instanceof CommandError ? error.code : undefined;
}
