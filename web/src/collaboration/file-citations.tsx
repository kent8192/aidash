import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  addFileCitation,
  removeFileCitation,
  type FileCitation,
} from "./file-citation-model";

interface FileCitationContextValue {
  citationsFor: (target: string) => readonly FileCitation[];
  add: (target: string, citation: FileCitation) => void;
  remove: (target: string, key: string) => void;
  clear: (target: string) => void;
}

const empty: readonly FileCitation[] = [];
const FileCitationContext = createContext<FileCitationContextValue>({
  citationsFor: () => empty,
  add: () => undefined,
  remove: () => undefined,
  clear: () => undefined,
});

export function useFileCitations(): FileCitationContextValue {
  return useContext(FileCitationContext);
}

/**
 * Composer citations are keyed by the message target ("channel" or a thread
 * id), so quoting a File version in one thread does not leak into another.
 */
export function FileCitationProvider({ children }: { children: ReactNode }) {
  const [byTarget, setByTarget] = useState<Record<string, FileCitation[]>>({});
  const citationsFor = useCallback(
    (target: string) => byTarget[target] ?? empty,
    [byTarget],
  );
  const add = useCallback((target: string, citation: FileCitation) => {
    setByTarget((current) => {
      const next = addFileCitation(current[target] ?? empty, citation);
      if (next === (current[target] ?? empty)) return current;
      return { ...current, [target]: [...next] };
    });
  }, []);
  const remove = useCallback((target: string, key: string) => {
    setByTarget((current) => {
      const list = current[target] ?? empty;
      const next = removeFileCitation(list, key);
      if (next === list) return current;
      return { ...current, [target]: [...next] };
    });
  }, []);
  const clear = useCallback((target: string) => {
    setByTarget((current) => {
      if (!current[target]?.length) return current;
      return { ...current, [target]: [] };
    });
  }, []);
  const value = useMemo(
    () => ({ citationsFor, add, remove, clear }),
    [citationsFor, add, remove, clear],
  );
  return (
    <FileCitationContext.Provider value={value}>
      {children}
    </FileCitationContext.Provider>
  );
}
