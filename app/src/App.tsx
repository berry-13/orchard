import { lazy, Suspense } from "react"

import { AddWifiDialog, AppSidebar } from "@/components/app-sidebar"
import { CommandPalette } from "@/components/command-palette"
import { DropOverlay, IpaInstallSheet } from "@/components/ipa-install"
import { TaskAnnouncer } from "@/components/task-tray"
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar"
import { Toaster } from "@/components/ui/sonner"
import { TooltipProvider } from "@/components/ui/tooltip"
import { DeviceProvider } from "@/lib/devices"
import { NavProvider, useNav, type Section } from "@/lib/nav"
import { TaskProvider } from "@/lib/tasks"
import { UiProvider } from "@/lib/ui"

const screens: Record<Section, React.LazyExoticComponent<() => React.JSX.Element>> = {
  overview: lazy(() => import("@/screens/overview")),
  apps: lazy(() => import("@/screens/apps")),
  files: lazy(() => import("@/screens/files")),
  store: lazy(() => import("@/screens/app-store")),
  backups: lazy(() => import("@/screens/backups")),
  photos: lazy(() => import("@/screens/photos")),
  media: lazy(() => import("@/screens/media")),
  console: lazy(() => import("@/screens/console")),
  settings: lazy(() => import("@/screens/settings")),
}

function CurrentScreen() {
  const { section } = useNav()
  const Screen = screens[section]
  return (
    <Suspense fallback={<div className="flex-1" aria-busy="true" />}>
      <Screen key={section} />
    </Suspense>
  )
}

export function App() {
  return (
    <TooltipProvider delayDuration={300}>
      <NavProvider>
        <UiProvider>
          <TaskProvider>
            <DeviceProvider>
              <SidebarProvider className="h-full">
                <a
                  href="#main"
                  className="sr-only z-50 rounded-md bg-background px-3 py-2 text-sm focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:ring-2 focus:ring-ring"
                >
                  Skip to content
                </a>
                <AppSidebar />
                <SidebarInset id="main" className="min-w-0 overflow-y-auto">
                  <CurrentScreen />
                </SidebarInset>
              </SidebarProvider>
              <CommandPalette />
              <AddWifiDialog />
              <IpaInstallSheet />
              <DropOverlay />
              <TaskAnnouncer />
              <Toaster position="bottom-center" />
            </DeviceProvider>
          </TaskProvider>
        </UiProvider>
      </NavProvider>
    </TooltipProvider>
  )
}
