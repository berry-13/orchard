import { Laptop, Moon, PackagePlus, PenLine, Plus, Sun } from "lucide-react"
import { useEffect } from "react"

import { useInstallIpa } from "@/components/ipa-install"
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command"
import { DeviceGlyph } from "@/components/app-sidebar"
import { useDevices } from "@/lib/devices"
import { SECTIONS, useNav } from "@/lib/nav"
import { useTheme } from "@/lib/theme"
import { useUi } from "@/lib/ui"

export function CommandPalette() {
  const { paletteOpen, setPaletteOpen, setAddWifiOpen } = useUi()
  const { navigate } = useNav()
  const { devices, device, select } = useDevices()
  const { setTheme } = useTheme()
  const installIpa = useInstallIpa()
  const resignIpa = useInstallIpa("resign")

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault()
        setPaletteOpen(!paletteOpen)
      }
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [paletteOpen, setPaletteOpen])

  const run = (fn: () => void) => () => {
    setPaletteOpen(false)
    fn()
  }

  return (
    <CommandDialog
      open={paletteOpen}
      onOpenChange={setPaletteOpen}
      title="Command palette"
      description="Jump to a section or run an action"
    >
      <Command>
        <CommandInput placeholder="Type a command or search..." />
        <CommandList>
          <CommandEmpty>No results found.</CommandEmpty>
          <CommandGroup heading="Go to">
            {SECTIONS.map((s) => (
              <CommandItem key={s.id} value={`go ${s.title}`} onSelect={run(() => navigate(s.id))}>
                <s.icon aria-hidden="true" />
                {s.title}
              </CommandItem>
            ))}
          </CommandGroup>
          <CommandGroup heading="Actions">
            <CommandItem value="install ipa" disabled={!device} onSelect={run(() => void installIpa())}>
              <PackagePlus aria-hidden="true" />
              Install IPA...
            </CommandItem>
            <CommandItem value="resign ipa re-sign" onSelect={run(() => void resignIpa())}>
              <PenLine aria-hidden="true" />
              Re-sign IPA...
            </CommandItem>
            <CommandItem value="add wifi device" onSelect={run(() => setAddWifiOpen(true))}>
              <Plus aria-hidden="true" />
              Add Wi-Fi device...
            </CommandItem>
          </CommandGroup>
          {devices.length > 1 ? (
            <>
                  <CommandGroup heading="Switch device">
                {devices.map((d) => (
                  <CommandItem key={d.udid} value={`device ${d.name} ${d.udid}`} onSelect={run(() => select(d.udid))}>
                    <DeviceGlyph device={d} />
                    {d.name}
                  </CommandItem>
                ))}
              </CommandGroup>
            </>
          ) : null}
          <CommandGroup heading="Theme">
            <CommandItem value="theme light" onSelect={run(() => setTheme("light"))}>
              <Sun aria-hidden="true" />
              Light
            </CommandItem>
            <CommandItem value="theme dark" onSelect={run(() => setTheme("dark"))}>
              <Moon aria-hidden="true" />
              Dark
            </CommandItem>
            <CommandItem value="theme system" onSelect={run(() => setTheme("system"))}>
              <Laptop aria-hidden="true" />
              System
            </CommandItem>
          </CommandGroup>
        </CommandList>
      </Command>
    </CommandDialog>
  )
}
