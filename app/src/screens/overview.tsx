import { useMutation, useQueryClient } from "@tanstack/react-query"
import {
  BatteryCharging,
  BatteryFull,
  BatteryLow,
  BatteryMedium,
  Camera,
  Check,
  Copy,
  Download,
  HardDrive,
  Link2,
  Link2Off,
  Power,
  RotateCcw,
} from "lucide-react"
import { useState } from "react"
import { toast } from "sonner"

import { ConnectionBadge, DeviceGlyph } from "@/components/app-sidebar"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { NoDevice } from "@/components/no-device"
import { PageBody, PageHeader } from "@/components/page-header"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Label } from "@/components/ui/label"
import { Progress } from "@/components/ui/progress"
import { Skeleton } from "@/components/ui/skeleton"
import { Switch } from "@/components/ui/switch"
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip"
import {
  currentBackend,
  errorMessage,
  pairDevice,
  pickSavePath,
  restartDevice,
  revealPath,
  saveScreenshot,
  setWifiSync,
  shutdownDevice,
  takeScreenshot,
  unpairDevice,
  type DeviceInfo,
} from "@/lib/api"
import { deviceKeys, useDeviceInfo, useDevices } from "@/lib/devices"
import { formatBytes } from "@/lib/format"

function CopyValue({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <span className="flex min-w-0 items-center gap-1">
      <span className="truncate font-mono text-xs">{value}</span>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={`Copy ${label}`}
            onClick={() => {
              void navigator.clipboard.writeText(value).then(() => {
                setCopied(true)
                toast.success(`${label} copied`)
                setTimeout(() => setCopied(false), 1500)
              })
            }}
          >
            {copied ? <Check /> : <Copy />}
          </Button>
        </TooltipTrigger>
        <TooltipContent>Copy {label}</TooltipContent>
      </Tooltip>
    </span>
  )
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[8rem_1fr] items-center gap-3 py-2.5 text-sm">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate">{children}</dd>
    </div>
  )
}

function BatteryIcon({ level, charging }: { level: number | null; charging: boolean }) {
  const cls = "size-4 text-muted-foreground"
  if (charging) return <BatteryCharging className={cls} aria-hidden="true" />
  if (level === null || level > 70) return <BatteryFull className={cls} aria-hidden="true" />
  if (level > 25) return <BatteryMedium className={cls} aria-hidden="true" />
  return <BatteryLow className="size-4 text-destructive" aria-hidden="true" />
}

function ScreenshotDialog({
  device,
  open,
  onOpenChange,
}: {
  device: DeviceInfo
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const shot = useMutation({ mutationFn: () => takeScreenshot(device.udid) })
  const [prevOpen, setPrevOpen] = useState(open)
  if (open !== prevOpen) {
    setPrevOpen(open)
    if (open) shot.mutate()
  }

  const save = async () => {
    if (!shot.data) return
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-")
    const path = await pickSavePath("Save screenshot", `${device.name} ${stamp}.png`)
    if (!path) return
    try {
      await saveScreenshot(path, shot.data)
      if (currentBackend() === "remote") await revealPath(path)
      else toast.success("Screenshot saved", { description: path })
    } catch (e) {
      toast.error("Could not save screenshot", { description: errorMessage(e) })
    }
  }

  const src = shot.data
    ? shot.data.startsWith("iVBOR")
      ? `data:image/png;base64,${shot.data}`
      : `data:image/svg+xml;base64,${shot.data}`
    : null

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Screenshot</DialogTitle>
          <DialogDescription>{device.name}</DialogDescription>
        </DialogHeader>
        <div className="flex min-h-80 items-center justify-center rounded-lg border bg-muted/40 p-4">
          {shot.isPending ? (
            <Skeleton className="h-96 w-48 rounded-xl" aria-label="Capturing screenshot" />
          ) : shot.isError ? (
            <p className="text-sm text-destructive" role="alert">
              {errorMessage(shot.error)}
            </p>
          ) : src ? (
            <img
              src={src}
              alt={`Screenshot of ${device.name}`}
              className="max-h-[60vh] rounded-xl border shadow-sm animate-in fade-in-0 zoom-in-95 duration-200"
            />
          ) : null}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => shot.mutate()} disabled={shot.isPending}>
            <RotateCcw aria-hidden="true" />
            Retake
          </Button>
          <Button onClick={() => void save()} disabled={!shot.data}>
            <Download aria-hidden="true" />
            Save...
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function OverviewSkeleton() {
  return (
    <div className="grid gap-4 lg:grid-cols-3" aria-busy="true" aria-label="Loading device">
      <Skeleton className="h-72 lg:col-span-2" />
      <div className="grid gap-4">
        <Skeleton className="h-34" />
        <Skeleton className="h-34" />
      </div>
    </div>
  )
}

type Confirm = "restart" | "shutdown" | "unpair" | null

function DeviceOverview({ udid }: { udid: string }) {
  const { data: info, isPending, isError, error } = useDeviceInfo(udid)
  const queryClient = useQueryClient()
  const [shotOpen, setShotOpen] = useState(false)
  const [confirm, setConfirm] = useState<Confirm>(null)

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: deviceKeys.info(udid) })
    void queryClient.invalidateQueries({ queryKey: deviceKeys.list })
  }

  const wifiSync = useMutation({
    mutationFn: (enabled: boolean) => setWifiSync(udid, enabled),
    onMutate: (enabled) => {
      queryClient.setQueryData<DeviceInfo>(deviceKeys.info(udid), (d) => d && { ...d, wifiSyncEnabled: enabled })
    },
    onSuccess: (_, enabled) => toast.success(enabled ? "Wi-Fi sync turned on" : "Wi-Fi sync turned off"),
    onError: (e) => {
      toast.error("Could not change Wi-Fi sync", { description: errorMessage(e) })
      refresh()
    },
  })

  const action = useMutation({
    mutationFn: async (kind: Exclude<Confirm, null> | "pair") => {
      if (kind === "restart") await restartDevice(udid)
      else if (kind === "shutdown") await shutdownDevice(udid)
      else if (kind === "unpair") await unpairDevice(udid)
      else await pairDevice(udid)
      return kind
    },
    onSuccess: (kind) => {
      const messages = {
        restart: "Restarting device",
        shutdown: "Shutting down device",
        unpair: "Device unpaired",
        pair: "Device paired",
      }
      toast.success(messages[kind])
      refresh()
    },
    onError: (e) => toast.error("Action failed", { description: errorMessage(e) }),
  })

  if (isPending) return <OverviewSkeleton />
  if (isError) {
    return (
      <p className="text-sm text-destructive" role="alert">
        {errorMessage(error)}
      </p>
    )
  }

  const storagePercent = info.storageTotal ? Math.round((info.storageUsed / info.storageTotal) * 100) : 0
  const free = info.storageTotal - info.storageUsed

  return (
    <div className="grid gap-4 lg:grid-cols-3 animate-in fade-in-0 duration-200">
      <Card className="lg:col-span-2">
        <CardHeader className="flex flex-row items-start gap-4">
          <div className="flex size-12 shrink-0 items-center justify-center rounded-xl border bg-muted/50">
            <DeviceGlyph device={info} className="size-6" />
          </div>
          <div className="grid min-w-0 flex-1 gap-1">
            <CardTitle className="flex items-center gap-2 text-base">
              <span className="truncate">{info.name}</span>
              <ConnectionBadge connection={info.connection} />
              {info.paired ? null : <Badge variant="destructive">Not paired</Badge>}
            </CardTitle>
            <CardDescription className="truncate">
              {info.model} · iOS {info.iosVersion}
            </CardDescription>
          </div>
        </CardHeader>
        <CardContent>
          <dl className="divide-y border-t">
            <Row label="Model">
              {info.model} <span className="text-muted-foreground">({info.productType})</span>
            </Row>
            <Row label="iOS">
              {info.iosVersion} <span className="text-muted-foreground">({info.buildVersion})</span>
            </Row>
            <Row label="Serial number">
              <CopyValue value={info.serialNumber} label="serial number" />
            </Row>
            <Row label="UDID">
              <CopyValue value={info.udid} label="UDID" />
            </Row>
          </dl>
        </CardContent>
      </Card>

      <div className="grid content-start gap-4">
        <Card>
          <CardHeader>
            <CardDescription className="flex items-center gap-2">
              <BatteryIcon level={info.batteryLevel} charging={info.batteryCharging} />
              Battery
            </CardDescription>
            <CardTitle className="text-2xl font-semibold tabular-nums">
              {info.batteryLevel === null ? "Unknown" : `${info.batteryLevel}%`}
            </CardTitle>
          </CardHeader>
          <CardContent className="grid gap-2">
            <Progress value={info.batteryLevel ?? 0} aria-label="Battery level" className="h-1.5" />
            <p className="text-xs text-muted-foreground">{info.batteryCharging ? "Charging" : "On battery"}</p>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="flex items-center gap-2">
              <HardDrive className="size-4" aria-hidden="true" />
              Storage
            </CardDescription>
            <CardTitle className="text-2xl font-semibold tabular-nums">
              {formatBytes(info.storageUsed)}
              <span className="text-sm font-normal text-muted-foreground"> of {formatBytes(info.storageTotal, 0)}</span>
            </CardTitle>
          </CardHeader>
          <CardContent className="grid gap-2">
            <Progress value={storagePercent} aria-label="Storage used" className="h-1.5" />
            <p className="text-xs text-muted-foreground tabular-nums">
              {formatBytes(free)} available · {storagePercent}% used
            </p>
          </CardContent>
        </Card>
      </div>

      <Card className="lg:col-span-2">
        <CardHeader>
          <CardTitle className="text-sm">Connection</CardTitle>
          <CardDescription>Wi-Fi sync lets Orchard reach this device without a cable.</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4">
          <div className="flex items-center justify-between gap-4 rounded-lg border p-3">
            <div className="grid gap-0.5">
              <Label htmlFor="wifi-sync">Wi-Fi sync</Label>
              <p id="wifi-sync-hint" className="text-xs text-muted-foreground">
                Show this device when it is on the same network.
              </p>
            </div>
            <Switch
              id="wifi-sync"
              aria-describedby="wifi-sync-hint"
              checked={info.wifiSyncEnabled}
              disabled={wifiSync.isPending}
              onCheckedChange={(v) => wifiSync.mutate(v)}
            />
          </div>
          <div className="flex items-center justify-between gap-4 rounded-lg border p-3">
            <div className="grid gap-0.5">
              <p className="text-sm font-medium">Pairing</p>
              <p className="text-xs text-muted-foreground">
                {info.paired
                  ? "This computer is trusted by the device."
                  : "Pair over USB, then tap Trust on the device."}
              </p>
            </div>
            {info.paired ? (
              <Button variant="outline" size="sm" onClick={() => setConfirm("unpair")}>
                <Link2Off aria-hidden="true" />
                Unpair
              </Button>
            ) : (
              <Button
                size="sm"
                onClick={() => action.mutate("pair")}
                disabled={action.isPending || info.connection !== "usb"}
              >
                <Link2 aria-hidden="true" />
                Pair
              </Button>
            )}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Actions</CardTitle>
          <CardDescription>Capture the screen or control power.</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-2">
          <Button variant="outline" className="justify-start" onClick={() => setShotOpen(true)}>
            <Camera aria-hidden="true" />
            Take screenshot
          </Button>
          <Button variant="outline" className="justify-start" onClick={() => setConfirm("restart")}>
            <RotateCcw aria-hidden="true" />
            Restart
          </Button>
          <Button
            variant="outline"
            className="justify-start text-destructive hover:text-destructive"
            onClick={() => setConfirm("shutdown")}
          >
            <Power aria-hidden="true" />
            Shut down
          </Button>
        </CardContent>
      </Card>

      <ScreenshotDialog device={info} open={shotOpen} onOpenChange={setShotOpen} />
      <ConfirmDialog
        open={confirm === "restart"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={`Restart ${info.name}?`}
        description="The device disconnects and comes back after it boots. Running transfers will fail."
        actionLabel="Restart"
        destructive={false}
        onConfirm={() => action.mutate("restart")}
      />
      <ConfirmDialog
        open={confirm === "shutdown"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={`Shut down ${info.name}?`}
        description="You will need to turn it on with its buttons."
        actionLabel="Shut down"
        onConfirm={() => action.mutate("shutdown")}
      />
      <ConfirmDialog
        open={confirm === "unpair"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title="Unpair this device?"
        description="Orchard forgets the pairing record. The device has to trust this computer again over USB."
        actionLabel="Unpair"
        onConfirm={() => action.mutate("unpair")}
      />
    </div>
  )
}

export default function OverviewScreen() {
  const { device, loading } = useDevices()
  return (
    <>
      <PageHeader title="Overview" description={device ? device.name : undefined} />
      <PageBody>
        {loading ? <OverviewSkeleton /> : device ? <DeviceOverview udid={device.udid} /> : <NoDevice />}
      </PageBody>
    </>
  )
}
