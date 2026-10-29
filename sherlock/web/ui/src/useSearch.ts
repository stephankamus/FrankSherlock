import { useCallback, useEffect, useRef, useState } from "react";
import {
  errorMessage,
  useLibrary,
  type ParsedQuery,
  type SearchItem,
  type SortField,
  type SortOrder,
} from "@frank-sherlock/shared";

const PAGE_SIZE = 60;

type Params = {
  query: string;
  mediaType: string;
  rootId: number | null;
  sortBy: SortField;
  sortOrder: SortOrder;
  /** The faces/duplicates views pause the grid without dropping its results. */
  enabled: boolean;
};

export function useSearch({ query, mediaType, rootId, sortBy, sortOrder, enabled }: Params) {
  const library = useLibrary();
  const [items, setItems] = useState<SearchItem[]>([]);
  const [total, setTotal] = useState(0);
  const [parsedQuery, setParsedQuery] = useState<ParsedQuery | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);

  const run = useCallback(
    async (offset: number, append: boolean) => {
      const id = ++requestId.current;
      if (append) setLoadingMore(true);
      else setLoading(true);
      try {
        const response = await library.search({
          query,
          limit: PAGE_SIZE,
          offset,
          mediaTypes: mediaType ? [mediaType] : undefined,
          rootScope: rootId != null ? [rootId] : undefined,
          sortBy,
          sortOrder,
        });
        if (id !== requestId.current) return;
        setTotal(response.total);
        setParsedQuery(response.parsedQuery);
        setError(null);
        setItems((prev) => (append ? [...prev, ...response.items] : response.items));
      } catch (err) {
        if (id !== requestId.current) return;
        setError(errorMessage(err));
        if (!append) {
          setItems([]);
          setTotal(0);
        }
      } finally {
        if (id === requestId.current) {
          setLoading(false);
          setLoadingMore(false);
        }
      }
    },
    [library, query, mediaType, rootId, sortBy, sortOrder],
  );

  // Same debounce as the desktop: typing should not fire one request per key.
  useEffect(() => {
    if (!enabled) return;
    const timer = setTimeout(() => {
      void run(0, false);
    }, 250);
    return () => clearTimeout(timer);
  }, [enabled, run]);

  const loadMore = useCallback(async () => {
    if (loading || loadingMore || items.length >= total) return;
    await run(items.length, true);
  }, [loading, loadingMore, items.length, total, run]);

  return {
    items,
    total,
    parsedQuery,
    loading,
    loadingMore,
    error,
    loadMore,
    canLoadMore: items.length < total,
  };
}
