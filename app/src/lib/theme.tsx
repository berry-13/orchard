import { createContext, useCallback, useContext, useEffect, useMemo, useSyncExternalStore, useState } from "react"

import { isTauri } from "@tauri-apps/api/core"
import { getCurrentWindow } from "@tauri-apps/api/window"

import { readStored, writeStored } from "@/lib/storage"

export type Theme = "light" | "dark" | "system"

const STORAGE_KEY = "orchard.theme"
const media = window.matchMedia("(prefers-color-scheme: dark)")

function storedTheme(): Theme {
  const value = readStored(STORAGE_KEY)
  return value === "light" || value === "dark" || value === "system" ? value : "system"
}

function apply(theme: Theme) {
  const dark = theme === "dark" || (theme === "system" && media.matches)
  document.documentElement.classList.toggle("dark", dark)
  document.documentElement.style.colorScheme = dark ? "dark" : "light"
  // Native materials and title bars follow the window theme, not the page.
  if (isTauri()) getCurrentWindow().setTheme(theme === "system" ? null : theme).catch(() => {})
}

apply(storedTheme())

function subscribeSystem(cb: () => void) {
  media.addEventListener("change", cb)
  return () => media.removeEventListener("change", cb)
}

interface ThemeContextValue {
  theme: Theme
  resolved: "light" | "dark"
  setTheme: (theme: Theme) => void
}

const ThemeContext = createContext<ThemeContextValue | null>(null)

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(storedTheme)
  const systemDark = useSyncExternalStore(subscribeSystem, () => media.matches)
  const resolved = theme === "system" ? (systemDark ? "dark" : "light") : theme

  const setTheme = useCallback((next: Theme) => {
    writeStored(STORAGE_KEY, next)
    apply(next)
    setThemeState(next)
  }, [])

  useEffect(() => apply(theme), [theme, resolved])

  const value = useMemo(() => ({ theme, resolved, setTheme }), [theme, resolved, setTheme])
  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}

export function useTheme() {
  const ctx = useContext(ThemeContext)
  if (!ctx) throw new Error("useTheme must be used inside ThemeProvider")
  return ctx
}
