import { createContext, useCallback, useContext, useMemo, useState } from "react"

/** What the IPA sheet does with the chosen file. */
export type IpaMode = "install" | "resign"

interface UiContextValue {
  paletteOpen: boolean
  setPaletteOpen: (open: boolean) => void
  addWifiOpen: boolean
  setAddWifiOpen: (open: boolean) => void
  /** IPA path shown in the IPA sheet, null when closed. */
  ipaPath: string | null
  ipaMode: IpaMode
  /** Opens the IPA sheet for `path` (install by default), or closes it with null. */
  setIpaPath: (path: string | null, mode?: IpaMode) => void
}

const UiContext = createContext<UiContextValue | null>(null)

/** Window level dialogs that can be opened from anywhere (palette, sidebar, drop). */
export function UiProvider({ children }: { children: React.ReactNode }) {
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [addWifiOpen, setAddWifiOpen] = useState(false)
  const [ipa, setIpa] = useState<{ path: string | null; mode: IpaMode }>({ path: null, mode: "install" })
  const setIpaPath = useCallback(
    (path: string | null, mode: IpaMode = "install") =>
      setIpa((prev) => ({ path, mode: path ? mode : prev.mode })),
    [],
  )
  const value = useMemo(
    () => ({
      paletteOpen,
      setPaletteOpen,
      addWifiOpen,
      setAddWifiOpen,
      ipaPath: ipa.path,
      ipaMode: ipa.mode,
      setIpaPath,
    }),
    [paletteOpen, addWifiOpen, ipa, setIpaPath],
  )
  return <UiContext.Provider value={value}>{children}</UiContext.Provider>
}

export function useUi() {
  const ctx = useContext(UiContext)
  if (!ctx) throw new Error("useUi must be used inside UiProvider")
  return ctx
}
