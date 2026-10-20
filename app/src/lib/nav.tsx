import {
  Archive,
  Clapperboard,
  Cog,
  FolderOpen,
  Images,
  LayoutGrid,
  Smartphone,
  ShoppingBag,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react"
import { createContext, useContext, useMemo, useState } from "react"

import { readStored, writeStored } from "@/lib/storage"

export type Section =
  | "overview"
  | "apps"
  | "files"
  | "store"
  | "backups"
  | "photos"
  | "media"
  | "console"
  | "settings"

export interface SectionMeta {
  id: Section
  title: string
  icon: LucideIcon
  /** Needs a connected device to be useful. */
  needsDevice: boolean
}

export const SECTIONS: SectionMeta[] = [
  { id: "overview", title: "Overview", icon: Smartphone, needsDevice: true },
  { id: "apps", title: "Apps", icon: LayoutGrid, needsDevice: true },
  { id: "files", title: "Files", icon: FolderOpen, needsDevice: true },
  { id: "store", title: "App Store", icon: ShoppingBag, needsDevice: false },
  { id: "backups", title: "Backups", icon: Archive, needsDevice: false },
  { id: "photos", title: "Photos", icon: Images, needsDevice: true },
  { id: "media", title: "Media", icon: Clapperboard, needsDevice: true },
  { id: "console", title: "Console", icon: SquareTerminal, needsDevice: true },
  { id: "settings", title: "Settings", icon: Cog, needsDevice: false },
]

const STORAGE_KEY = "orchard.section"

interface NavContextValue {
  section: Section
  navigate: (section: Section) => void
}

const NavContext = createContext<NavContextValue | null>(null)

function storedSection(): Section {
  const value = readStored(STORAGE_KEY)
  return SECTIONS.some((s) => s.id === value) ? (value as Section) : "overview"
}

export function NavProvider({ children }: { children: React.ReactNode }) {
  const [section, setSection] = useState<Section>(() => {
    const fromHash = window.location.hash.slice(1)
    return SECTIONS.some((s) => s.id === fromHash) ? (fromHash as Section) : storedSection()
  })
  const value = useMemo<NavContextValue>(
    () => ({
      section,
      navigate: (next) => {
        writeStored(STORAGE_KEY, next)
        setSection(next)
      },
    }),
    [section],
  )
  return <NavContext.Provider value={value}>{children}</NavContext.Provider>
}

export function useNav() {
  const ctx = useContext(NavContext)
  if (!ctx) throw new Error("useNav must be used inside NavProvider")
  return ctx
}
