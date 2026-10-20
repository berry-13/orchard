import { useQueryClient } from "@tanstack/react-query"
import { Archive, Lock } from "lucide-react"
import { useState } from "react"

import { DeviceGlyph } from "@/components/app-sidebar"
import { EmptyState } from "@/components/empty-state"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Label } from "@/components/ui/label"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { Skeleton } from "@/components/ui/skeleton"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { createBackup, type BackupMode, type BackupSummary, type Device } from "@/lib/api"
import { formatBytes, formatDateTime, formatRelative } from "@/lib/format"
import { useTasks } from "@/lib/tasks"

export function BackupTable({
  backups,
  loading,
  onOpen,
}: {
  backups: BackupSummary[] | undefined
  loading: boolean
  onOpen: (backup: BackupSummary) => void
}) {
  if (!loading && backups?.length === 0) {
    return (
      <EmptyState
        icon={Archive}
        title="No backups yet"
        description="Back up a connected device to keep a local copy of its data."
      />
    )
  }

  return (
    <div className="overflow-hidden rounded-lg border">
      <Table>
        <TableHeader className="bg-muted/40">
          <TableRow>
            <TableHead className="pl-4">Device</TableHead>
            <TableHead>Last backup</TableHead>
            <TableHead>iOS</TableHead>
            <TableHead className="w-10">
              <span className="sr-only">Encryption</span>
            </TableHead>
            <TableHead className="pr-4 text-right">Size</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {loading || !backups
            ? Array.from({ length: 3 }, (_, i) => (
                <TableRow key={i}>
                  <TableCell className="pl-4">
                    <div className="flex items-center gap-3">
                      <Skeleton className="size-8 rounded-lg" />
                      <div className="grid gap-1.5">
                        <Skeleton className="h-3.5 w-32" />
                        <Skeleton className="h-3 w-20" />
                      </div>
                    </div>
                  </TableCell>
                  <TableCell>
                    <Skeleton className="h-3.5 w-24" />
                  </TableCell>
                  <TableCell>
                    <Skeleton className="h-3.5 w-12" />
                  </TableCell>
                  <TableCell />
                  <TableCell className="pr-4">
                    <Skeleton className="ml-auto h-3.5 w-14" />
                  </TableCell>
                </TableRow>
              ))
            : backups.map((b) => (
                <TableRow
                  key={b.id}
                  className="cursor-pointer"
                  onClick={() => onOpen(b)}
                >
                  <TableCell className="pl-4">
                    <div className="flex items-center gap-3">
                      <div className="flex size-8 shrink-0 items-center justify-center rounded-lg border bg-muted/40">
                        <DeviceGlyph device={b} className="size-4" />
                      </div>
                      <div className="grid min-w-0">
                        <button
                          type="button"
                          className="truncate rounded-sm text-left font-medium outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
                          onClick={(e) => {
                            e.stopPropagation()
                            onOpen(b)
                          }}
                          aria-label={`Open backup of ${b.deviceName}`}
                        >
                          {b.deviceName}
                        </button>
                        <span className="truncate text-xs text-muted-foreground">{b.productType}</span>
                      </div>
                    </div>
                  </TableCell>
                  <TableCell>
                    <time dateTime={new Date(b.date).toISOString()} title={formatDateTime(b.date)}>
                      {formatRelative(b.date)}
                    </time>
                  </TableCell>
                  <TableCell className="tabular-nums">{b.iosVersion}</TableCell>
                  <TableCell>
                    {b.encrypted ? (
                      <Lock className="size-4 text-muted-foreground" aria-label="Encrypted" role="img" />
                    ) : null}
                  </TableCell>
                  <TableCell className="pr-4 text-right tabular-nums">{formatBytes(b.size)}</TableCell>
                </TableRow>
              ))}
        </TableBody>
      </Table>
    </div>
  )
}

export function CreateBackupDialog({
  device,
  open,
  onOpenChange,
}: {
  device: Device
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const [mode, setMode] = useState<BackupMode>("incremental")
  const tasks = useTasks()
  const queryClient = useQueryClient()

  const start = () => {
    onOpenChange(false)
    const udid = device.udid
    void tasks
      .start({
        kind: "backup",
        title: `Back up ${device.name}`,
        successMessage: `${device.name} backed up`,
        run: (taskId) => createBackup(taskId, udid, mode),
      })
      .then(() => queryClient.invalidateQueries({ queryKey: ["backups"] }))
  }

  const options: { value: BackupMode; title: string; hint: string }[] = [
    {
      value: "incremental",
      title: "Incremental",
      hint: "Only copies what changed since the last backup. Fastest.",
    },
    { value: "full", title: "Full", hint: "Copies everything again from scratch." },
  ]

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Back up {device.name}</DialogTitle>
          <DialogDescription>Keep the device connected and unlocked until the backup finishes.</DialogDescription>
        </DialogHeader>
        <RadioGroup
          value={mode}
          onValueChange={(v) => setMode(v as BackupMode)}
          aria-label="Backup type"
          className="gap-2"
        >
          {options.map((o) => (
            <Label
              key={o.value}
              htmlFor={`backup-${o.value}`}
              className="flex cursor-pointer items-start gap-3 rounded-lg border p-3 font-normal transition-colors duration-150 hover:bg-muted/50 has-data-checked:border-primary has-data-checked:bg-muted/50"
            >
              <RadioGroupItem id={`backup-${o.value}`} value={o.value} className="mt-0.5" />
              <span className="grid gap-0.5">
                <span className="text-sm font-medium">{o.title}</span>
                <span className="text-xs text-muted-foreground">{o.hint}</span>
              </span>
            </Label>
          ))}
        </RadioGroup>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={start}>
            <Archive aria-hidden="true" />
            Start backup
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
