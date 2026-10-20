import { useMutation, useQueryClient } from "@tanstack/react-query"
import {
  Cable,
  Check,
  ChevronsUpDown,
  Command as CommandIcon,
  Link2,
  Plus,
  Search,
  Smartphone,
  Tablet,
  Wifi,
} from "lucide-react"
import { useState } from "react"
import { toast } from "sonner"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSkeleton,
  SidebarRail,
} from "@/components/ui/sidebar"
import { canPairOverWifi, WifiPairButton } from "@/components/wifi-pair-dialog"
import { addWifiDevice, errorMessage, pairDevice, type Connection, type Device } from "@/lib/api"
import { deviceKeys, useDevices } from "@/lib/devices"
import { SECTIONS, useNav } from "@/lib/nav"
import { overlayTitleBar } from "@/lib/platform"
import { useUi } from "@/lib/ui"

export function DeviceGlyph({ device, className }: { device: Pick<Device, "productType">; className?: string }) {
  const Icon = device.productType.startsWith("iPad") ? Tablet : Smartphone
  return <Icon className={className} aria-hidden="true" />
}

export function ConnectionBadge({ connection }: { connection: Connection }) {
  return (
    <Badge variant="outline" className="gap-1 px-1.5 font-normal text-muted-foreground">
      {connection === "usb" ? <Cable aria-hidden="true" /> : <Wifi aria-hidden="true" />}
      {connection === "usb" ? "USB" : "Wi-Fi"}
    </Badge>
  )
}

function DeviceSwitcher() {
  const { devices, device, loading, select } = useDevices()
  const { setAddWifiOpen } = useUi()
  const queryClient = useQueryClient()
  const pair = useMutation({
    mutationFn: (udid: string) => pairDevice(udid),
    onSuccess: () => {
      toast.success("Device paired")
      void queryClient.invalidateQueries({ queryKey: deviceKeys.list })
    },
    onError: (e) => toast.error("Pairing failed", { description: errorMessage(e) }),
  })

  if (loading) return <SidebarMenuSkeleton showIcon />

  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <SidebarMenuButton
              size="lg"
              className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground"
            >
              <div className="flex aspect-square size-8 items-center justify-center rounded-lg border bg-background text-foreground">
                {device ? <DeviceGlyph device={device} className="size-4" /> : <Cable className="size-4" />}
              </div>
              <div className="grid flex-1 text-left text-sm leading-tight">
                <span className="truncate font-medium">{device?.name ?? "No device"}</span>
                <span className="truncate text-xs text-muted-foreground">
                  {device
                    ? `${device.model} · ${device.connection === "usb" ? "USB" : "Wi-Fi"}`
                    : "Connect with a cable"}
                </span>
              </div>
              <span className="sr-only">, switch device</span>
              <ChevronsUpDown className="ml-auto size-4 text-muted-foreground" aria-hidden="true" />
            </SidebarMenuButton>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            className="w-(--radix-dropdown-menu-trigger-width) min-w-64 rounded-lg"
            align="start"
            side="bottom"
            sideOffset={4}
          >
            <DropdownMenuLabel className="text-xs text-muted-foreground">Devices</DropdownMenuLabel>
            {devices.length === 0 ? (
              <p className="px-2 py-1.5 text-sm text-muted-foreground">No devices found</p>
            ) : (
              devices.map((d) => (
                <DropdownMenuItem key={d.udid} onSelect={() => select(d.udid)} className="gap-2 p-2">
                  <div className="flex size-6 items-center justify-center rounded-md border">
                    <DeviceGlyph device={d} className="size-3.5" />
                  </div>
                  <div className="grid min-w-0 flex-1 leading-tight">
                    <span className="truncate">{d.name}</span>
                    <span className="truncate text-xs text-muted-foreground">
                      iOS {d.iosVersion}
                      {d.paired ? "" : " · not paired"}
                    </span>
                  </div>
                  <ConnectionBadge connection={d.connection} />
                  {d.udid === device?.udid ? (
                    <Check className="size-4" aria-label="Selected" />
                  ) : (
                    <span className="size-4" />
                  )}
                </DropdownMenuItem>
              ))
            )}
            <DropdownMenuSeparator />
            <DropdownMenuItem className="gap-2 p-2" onSelect={() => setAddWifiOpen(true)}>
              <div className="flex size-6 items-center justify-center rounded-md border bg-transparent">
                <Plus className="size-4" aria-hidden="true" />
              </div>
              Add Wi-Fi device
            </DropdownMenuItem>
            <DropdownMenuItem
              className="gap-2 p-2"
              disabled={!device || device.connection !== "usb" || pair.isPending}
              onSelect={() => device && pair.mutate(device.udid)}
            >
              <div className="flex size-6 items-center justify-center rounded-md border bg-transparent">
                <Link2 className="size-4" aria-hidden="true" />
              </div>
              {pair.isPending ? "Pairing..." : device?.paired ? "Pair again" : "Pair"}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </SidebarMenuItem>
    </SidebarMenu>
  )
}

export function AddWifiDialog() {
  const { addWifiOpen, setAddWifiOpen } = useUi()
  const { select } = useDevices()
  const queryClient = useQueryClient()
  const [address, setAddress] = useState("")
  const add = useMutation({
    mutationFn: () => addWifiDevice(address.trim()),
    onSuccess: (device) => {
      toast.success(`Connected to ${device.name}`)
      void queryClient.invalidateQueries({ queryKey: deviceKeys.list })
      select(device.udid)
      setAddWifiOpen(false)
      setAddress("")
    },
  })

  return (
    <Dialog
      open={addWifiOpen}
      onOpenChange={(open) => {
        setAddWifiOpen(open)
        if (!open) add.reset()
      }}
    >
      <DialogContent className="sm:max-w-md">
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (address.trim()) add.mutate()
          }}
        >
          <DialogHeader>
            <DialogTitle>Add Wi-Fi device</DialogTitle>
            <DialogDescription>
              For a device that is already paired with this computer, on the same network.
            </DialogDescription>
          </DialogHeader>
          {canPairOverWifi() ? (
            <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border bg-muted/40 px-3 py-2">
              <p className="text-sm text-muted-foreground">Not paired yet?</p>
              <WifiPairButton />
            </div>
          ) : null}
          <div className="grid gap-2">
            <Label htmlFor="wifi-address">IP address or host name</Label>
            <Input
              id="wifi-address"
              placeholder="192.168.1.24"
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={add.isError || undefined}
              aria-describedby={add.isError ? "wifi-address-error" : undefined}
              autoFocus
            />
            {add.isError ? (
              <p id="wifi-address-error" className="text-sm text-destructive" role="alert">
                {errorMessage(add.error)}
              </p>
            ) : null}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setAddWifiOpen(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!address.trim() || add.isPending}>
              {add.isPending ? "Connecting..." : "Connect"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function AppSidebar() {
  const { section, navigate } = useNav()
  const { setPaletteOpen } = useUi()
  const main = SECTIONS.filter((s) => s.id !== "settings")
  const settings = SECTIONS.find((s) => s.id === "settings")!
  const isMac = navigator.platform.toLowerCase().includes("mac")

  return (
    <Sidebar collapsible="icon" aria-label="Main">
      <SidebarHeader data-tauri-drag-region={overlayTitleBar || undefined}>
        <DeviceSwitcher />
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <SidebarMenu>
              <SidebarMenuItem>
                <SidebarMenuButton
                  onClick={() => setPaletteOpen(true)}
                  tooltip="Search"
                  className="text-muted-foreground"
                >
                  <Search aria-hidden="true" />
                  <span>Search</span>
                  <kbd className="pointer-events-none ml-auto inline-flex h-5 items-center gap-0.5 rounded border bg-muted px-1.5 font-mono text-[10px] font-medium text-muted-foreground select-none">
                    {isMac ? <CommandIcon className="size-2.5" aria-label="Command" /> : "Ctrl "}K
                  </kbd>
                </SidebarMenuButton>
              </SidebarMenuItem>
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
        <SidebarGroup>
          <SidebarGroupLabel>Manage</SidebarGroupLabel>
          <SidebarGroupContent>
            <nav aria-label="Sections">
              <SidebarMenu>
                {main.map((item) => (
                  <SidebarMenuItem key={item.id}>
                    <SidebarMenuButton
                      isActive={section === item.id}
                      aria-current={section === item.id ? "page" : undefined}
                      tooltip={item.title}
                      onClick={() => navigate(item.id)}
                    >
                      <item.icon aria-hidden="true" />
                      <span>{item.title}</span>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </nav>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              isActive={section === "settings"}
              aria-current={section === "settings" ? "page" : undefined}
              tooltip={settings.title}
              onClick={() => navigate("settings")}
            >
              <settings.icon aria-hidden="true" />
              <span>{settings.title}</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  )
}
