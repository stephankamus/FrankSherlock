import { useEffect, useState } from "react";
import type { PersonInfo } from "../types";
import { useLibrary } from "../useLibrary";

const FACE_TOKEN = /\bface:(?:"[^"]*"|\S+)/gi;
const FACE_ID = /\bface:(\d+)(?!\S)/i;

/** The query with its `face:` token replaced by `face:<id>` (or removed for `null`). */
export function withPersonFilter(query: string, personId: number | null): string {
  const rest = query.replace(FACE_TOKEN, " ").replace(/\s+/g, " ").trim();
  if (personId == null) return rest;
  return rest ? `${rest} face:${personId}` : `face:${personId}`;
}

type Props = {
  query: string;
  onQueryChange: (value: string) => void;
};

/**
 * "People" filter of the library toolbar. It edits the `face:<id>` token of the search
 * query, so the filter, the query box and smart folders all stay one thing.
 */
export default function PeopleFilter({ query, onQueryChange }: Props) {
  const library = useLibrary();
  const [people, setPeople] = useState<PersonInfo[]>([]);

  useEffect(() => {
    let active = true;
    library.listPeople().then((list) => { if (active) setPeople(list); }).catch(() => {});
    return () => { active = false; };
  }, [library]);

  const match = FACE_ID.exec(query);
  const selected = match ? match[1] : "";
  if (people.length === 0 && !selected) return null;

  return (
    <select
      value={selected}
      onChange={(e) => onQueryChange(withPersonFilter(query, e.target.value ? Number(e.target.value) : null))}
      aria-label="People filter"
    >
      <option value="">all people</option>
      {selected && !people.some((p) => String(p.id) === selected) && (
        <option value={selected}>person {selected}</option>
      )}
      {people.map((p) => (
        <option key={p.id} value={String(p.id)}>
          {p.name} ({p.faceCount})
        </option>
      ))}
    </select>
  );
}
