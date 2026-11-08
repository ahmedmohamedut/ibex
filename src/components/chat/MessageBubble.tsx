import { File, FileQuestion, Pencil } from "lucide-react";
import { fileUrl } from "../../api/media";
import type { AttachmentRef } from "../../bindings/AttachmentRef";
import type { Message } from "../../bindings/Message";
import type { ReactionKind } from "../../bindings/ReactionKind";
import { formatBytes } from "../format";

/** A reaction as a short label. */
function reactionLabel(kind: ReactionKind): string {
  if (typeof kind === "string") {
    switch (kind) {
      case "loved":
        return "❤️";
      case "liked":
        return "👍";
      case "disliked":
        return "👎";
      case "laughed":
        return "😂";
      case "emphasized":
        return "‼️";
      case "questioned":
        return "❓";
    }
  }
  // iOS 18 custom emoji, or a code we do not recognise.
  if ("emoji" in kind) return kind.emoji;
  return "•";
}

function Attachment(props: {
  handle: string;
  attachment: AttachmentRef;
  onOpenImage: (url: string, name: string) => void;
}) {
  const { attachment: a } = props;

  if (!a.fileId) {
    // Listed in the database but never captured in the backup. Said plainly
    // rather than rendered as a broken image.
    return (
      <div className="attachment-card attachment-card--missing" title={a.fileName}>
        <FileQuestion size={14} aria-hidden="true" />
        <span className="attachment-card__name">{a.fileName}</span>
        <span className="attachment-card__size">Not in backup</span>
      </div>
    );
  }

  const url = fileUrl(props.handle, a.fileId);

  if (a.preview === "image") {
    return (
      <img
        className="attachment-image"
        src={url}
        alt={a.fileName}
        loading="lazy"
        onClick={() => props.onOpenImage(url, a.fileName)}
      />
    );
  }
  if (a.preview === "video") {
    // eslint-disable-next-line jsx-a11y/media-has-caption
    return <video className="attachment-image" src={url} controls preload="metadata" />;
  }
  if (a.preview === "audio") {
    // eslint-disable-next-line jsx-a11y/media-has-caption
    return <audio src={url} controls preload="metadata" style={{ maxWidth: 260 }} />;
  }

  return (
    <a className="attachment-card" href={url} download={a.fileName} title={a.fileName}>
      <File size={14} aria-hidden="true" />
      <span className="attachment-card__name">{a.fileName}</span>
      <span className="attachment-card__size">{formatBytes(a.size)}</span>
    </a>
  );
}

/**
 * One message.
 *
 * `palette` lets WhatsApp (M3) reuse this with its own bubble colours, which
 * already exist as tokens.
 */
export function MessageBubble(props: {
  handle: string;
  message: Message;
  /** Shown above the bubble in group chats. */
  showSender: boolean;
  /** The message this one replies to, when it is on screen. */
  parent?: Message;
  palette?: "imessage" | "whatsapp";
  /** Briefly flashed, e.g. when a search result lands on this message. */
  highlighted?: boolean;
  onOpenImage: (url: string, name: string) => void;
  onJumpTo: (id: string) => void;
}) {
  const { message: m } = props;
  const mine = m.fromMe;

  // An unsent or deleted message stays in the conversation flow as a dashed
  // bubble, so this check comes before the system one: WhatsApp marks deleted
  // messages in a way that would otherwise read as a system notice.
  if (m.kind === "system" && !m.unsent) {
    return (
      <div className="bubble-row bubble-row--system">
        <div className="bubble bubble--system">{m.body ?? "System message"}</div>
      </div>
    );
  }

  const bubbleClass = m.unsent
    ? "bubble bubble--unsent"
    : mine
      ? props.palette === "whatsapp"
        ? "bubble bubble--wa-me"
        : "bubble bubble--me"
      : "bubble bubble--them";

  const time = m.sentAt
    ? new Date(m.sentAt).toLocaleString(undefined, {
        hour: "2-digit",
        minute: "2-digit",
      })
    : "";

  const visible = m.reactions.filter((r) => !r.removed);

  return (
    <div
      className={
        (mine ? "bubble-row bubble-row--me" : "bubble-row bubble-row--them") +
        (props.highlighted ? " bubble-row--flash" : "")
      }
      id={`message-${m.id}`}
    >
      {props.showSender && !mine && m.sender ? (
        <div className="bubble__sender">
          {m.sender.displayName ?? m.sender.id}
        </div>
      ) : null}

      <div className={bubbleClass}>
        {m.replyTo ? (
          <button
            type="button"
            className="reply-quote"
            onClick={() => props.onJumpTo(m.replyTo as string)}
            title="Jump to the message this replies to"
          >
            {props.parent
              ? (props.parent.body ?? "Attachment")
              : "Replying to an earlier message"}
          </button>
        ) : null}

        {m.unsent ? (
          <em>{m.service === "whatsApp" ? "Message deleted" : "Message unsent"}</em>
        ) : (
          m.body
        )}

        {m.attachments.length > 0 ? (
          <div className="attachments">
            {m.attachments.map((a, index) => (
              <Attachment
                key={`${a.fileId ?? "missing"}-${index}`}
                handle={props.handle}
                attachment={a}
                onOpenImage={props.onOpenImage}
              />
            ))}
          </div>
        ) : null}
      </div>

      {visible.length > 0 ? (
        <div className="reactions">
          {visible.map((r, index) => (
            <span
              key={index}
              className="reaction"
              title={r.from?.displayName ?? r.from?.id ?? "You"}
            >
              {reactionLabel(r.kind)}
            </span>
          ))}
        </div>
      ) : null}

      <div className="bubble__meta">
        <span>{time}</span>
        {m.edited ? (
          <span title="Edited">
            <Pencil size={9} aria-hidden="true" /> edited
          </span>
        ) : null}
        {m.service === "sms" ? <span>SMS</span> : null}
        {m.service === "rcs" ? <span>RCS</span> : null}
      </div>
    </div>
  );
}
