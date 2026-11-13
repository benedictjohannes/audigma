import { useCallback, useEffect, useState } from "react";

export type Page = "form" | "review" | "save" | "guided";
const PAGES: Page[] = ["form", "review", "save", "guided"];

/** The page named by the hash (`#/review`), or `null` (no page chosen yet). No router library. */
export function pageFromHash(hash: string): Page | null {
  const name = hash.replace(/^#\/?/, "");
  return PAGES.find((p) => p === name) ?? null;
}

export function usePage(): [Page | null, (p: Page) => void] {
  const [page, setPage] = useState(() => pageFromHash(location.hash));
  useEffect(() => {
    const on = () => setPage(pageFromHash(location.hash));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  const goto = useCallback((p: Page) => {
    location.hash = `/${p}`;
  }, []);
  return [page, goto];
}
