export type Theme = "light" | "dark";
const KEY = "audigma-theme";

export function storedTheme(): Theme | null {
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" ? v : null;
  } catch {
    return null;
  }
}

export function storeTheme(t: Theme): void {
  try {
    localStorage.setItem(KEY, t);
  } catch {
    // no storage: the choice lasts for this page only
  }
}

export function systemTheme(): Theme {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  } catch {
    return "light";
  }
}

/** The theme in effect: the remembered choice, else the system setting. */
export function currentTheme(): Theme {
  return storedTheme() ?? systemTheme();
}

export function applyTheme(t: Theme): void {
  document.documentElement.dataset.theme = t;
}
