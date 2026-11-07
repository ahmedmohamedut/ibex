import {
  FileText,
  Folder,
  Image,
  LayoutDashboard,
  MessageCircle,
  MessageSquare,
  Mic,
  Share,
} from "lucide-react";
import { useDomains } from "../api/raw";
import { formatCount } from "./format";
import { useUi, type Section } from "../store/ui";

/** Sidebar entries that map onto a data source, in UI_DESIGN.md's order. */
const SOURCE_ITEMS: {
  section: Section;
  sourceId: string;
  label: string;
  Icon: typeof MessageSquare;
}[] = [
  { section: "messages", sourceId: "sms", label: "Messages", Icon: MessageSquare },
  { section: "whatsapp", sourceId: "whatsapp", label: "WhatsApp", Icon: MessageCircle },
  { section: "photos", sourceId: "photos", label: "Photos", Icon: Image },
  { section: "voice", sourceId: "voice", label: "Voice Memos", Icon: Mic },
  { section: "documents", sourceId: "documents", label: "Files", Icon: FileText },
];

export function Sidebar() {
  const backup = useUi((s) => s.backup);
  const section = useUi((s) => s.section);
  const setSection = useUi((s) => s.setSection);
  const domains = useDomains(backup?.handle);

  if (!backup) return null;

  const sourceById = new Map(backup.sources.map((s) => [s.id, s]));

  return (
    <nav className="sidebar" aria-label="Sections">
      <button
        type="button"
        className="sidebar__item"
        aria-current={section === "overview"}
        onClick={() => setSection("overview")}
      >
        <LayoutDashboard size={15} aria-hidden="true" />
        <span>Overview</span>
      </button>

      <div className="sidebar__group">Sources</div>
      {SOURCE_ITEMS.map(({ section: target, sourceId, label, Icon }) => {
        const source = sourceById.get(sourceId);
        // A section is usable only once its viewer has shipped *and* the data
        // is in this backup. The tooltip explains which half is missing.
        const usable = Boolean(source?.available && source?.implemented);
        const tooltip = source?.reason ?? "Not available in this backup";
        return (
          <button
            key={sourceId}
            type="button"
            className="sidebar__item"
            disabled={!usable}
            title={tooltip}
            aria-current={section === target}
            onClick={() => {
              if (usable) setSection(target);
            }}
          >
            <Icon size={15} aria-hidden="true" />
            <span>{label}</span>
            {source?.available && !source.implemented ? (
              <span className="sidebar__count">found</span>
            ) : null}
          </button>
        );
      })}

      <div className="sidebar__group">Everything</div>
      <button
        type="button"
        className="sidebar__item"
        aria-current={section === "raw"}
        onClick={() => setSection("raw")}
      >
        <Folder size={15} aria-hidden="true" />
        <span>Raw</span>
        <span className="sidebar__count">
          {domains.data ? formatCount(domains.data.length) : ""}
        </span>
      </button>
      <button
        type="button"
        className="sidebar__item"
        disabled
        title="Export history arrives in milestone M4"
      >
        <Share size={15} aria-hidden="true" />
        <span>Exports</span>
      </button>
    </nav>
  );
}
