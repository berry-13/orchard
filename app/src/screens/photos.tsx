import { useQuery } from "@tanstack/react-query"
import { Check, FileVideo, FolderOpen, Image, Images, Play, Share } from "lucide-react"
import { useMemo, useState } from "react"

import { EmptyState } from "@/components/empty-state"
import { NoDevice } from "@/components/no-device"
import { PageBody, PageHeader } from "@/components/page-header"
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
import { Skeleton } from "@/components/ui/skeleton"
import {
  errorMessage,
  exportPhotos,
  getSettings,
  listPhotos,
  pickFolder,
  type PhotoItem,
} from "@/lib/api"
import { useDevices } from "@/lib/devices"
import { formatBytes, formatDate, formatDateTime } from "@/lib/format"
import { useTasks } from "@/lib/tasks"
import { cn } from "@/lib/utils"

const GRID = "grid grid-cols-[repeat(auto-fill,minmax(8.5rem,1fr))] gap-3"

function PhotoTile({
  photo,
  selected,
  onToggle,
}: {
  photo: PhotoItem
  selected: boolean
  onToggle: () => void
}) {
  const video = photo.kind === "video"
  return (
    <li>
      <div
        role="checkbox"
        aria-checked={selected}
        aria-label={`${photo.name}, ${video ? "video" : "photo"}, ${formatDate(photo.modified)}, ${formatBytes(photo.size)}`}
        tabIndex={0}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === " " || e.key === "Enter") {
            e.preventDefault()
            onToggle()
          }
        }}
        className={cn(
          "group relative flex cursor-default flex-col overflow-hidden rounded-lg border bg-card text-left transition-[border-color,box-shadow] duration-150 outline-none select-none",
          "hover:border-foreground/20 focus-visible:ring-3 focus-visible:ring-ring/50",
          selected && "border-primary ring-1 ring-primary hover:border-primary",
        )}
      >
        <div className="relative flex aspect-square items-center justify-center bg-muted/60 text-muted-foreground">
          {video ? (
            <FileVideo className="size-7 opacity-60" aria-hidden="true" />
          ) : (
            <Image className="size-7 opacity-60" aria-hidden="true" />
          )}
          {video ? (
            <span className="absolute bottom-1.5 left-1.5 flex items-center gap-1 rounded bg-background/85 px-1.5 py-0.5 text-[10px] font-medium text-foreground">
              <Play className="size-2.5 fill-current" aria-hidden="true" />
              Video
            </span>
          ) : null}
          <span
            aria-hidden="true"
            className={cn(
              "absolute top-1.5 right-1.5 flex size-5 items-center justify-center rounded-full border bg-background/85 transition-opacity duration-150",
              selected
                ? "border-primary bg-primary text-primary-foreground opacity-100"
                : "opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100 pointer-coarse:opacity-100",
            )}
          >
            <Check className="size-3" />
          </span>
        </div>
        <div className="grid gap-0.5 border-t px-2.5 py-2">
          <span className="truncate text-xs font-medium" title={photo.path}>
            {photo.name}
          </span>
          <span className="flex justify-between gap-2 text-[11px] text-muted-foreground tabular-nums">
            <span className="truncate" title={formatDateTime(photo.modified)}>
              {formatDate(photo.modified)}
            </span>
            <span className="shrink-0">{formatBytes(photo.size)}</span>
          </span>
        </div>
      </div>
    </li>
  )
}

function ExportDialog({
  open,
  onOpenChange,
  count,
  bytes,
  onExport,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  count: number
  bytes: number
  onExport: (destDir: string) => void
}) {
  const settings = useQuery({ queryKey: ["settings"], queryFn: getSettings, enabled: open })
  const [chosen, setChosen] = useState<string | null>(null)
  const dest = chosen ?? settings.data?.downloadDir ?? null

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Export {count === 1 ? "1 item" : `${count} items`}</DialogTitle>
          <DialogDescription>
            {formatBytes(bytes)} will be copied from the device to a folder on this computer. Files
            already exported there are skipped.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <div className="grid gap-2">
            <Label id="export-dest-label">Destination</Label>
            <div className="flex items-center gap-2">
              <div
                aria-labelledby="export-dest-label"
                className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-lg border bg-muted/40 px-2.5 text-sm"
              >
                <FolderOpen className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                {settings.isPending && !chosen ? (
                  <Skeleton className="h-3.5 w-40" />
                ) : (
                  <span className={cn("truncate", !dest && "text-muted-foreground")} title={dest ?? undefined}>
                    {dest ?? "No folder chosen"}
                  </span>
                )}
              </div>
              <Button
                variant="outline"
                onClick={() => void pickFolder("Export to", dest).then((p) => p && setChosen(p))}
              >
                Change...
              </Button>
            </div>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={!dest} onClick={() => dest && onExport(dest)}>
            <Share aria-hidden="true" />
            Export
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function PhotoLibrary({ udid, deviceName }: { udid: string; deviceName: string }) {
  const tasks = useTasks()
  const photos = useQuery({ queryKey: ["photos", udid], queryFn: () => listPhotos(udid) })
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [exportOpen, setExportOpen] = useState(false)
  const items = useMemo(() => photos.data ?? [], [photos.data])

  const selectedItems = items.filter((p) => selected.has(p.path))
  const exportItems = selectedItems.length ? selectedItems : items
  const exportBytes = exportItems.reduce((sum, p) => sum + p.size, 0)
  const videos = items.filter((p) => p.kind === "video").length

  const toggle = (path: string) =>
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })

  const runExport = (destDir: string) => {
    const paths = exportItems.map((p) => p.path)
    setExportOpen(false)
    void tasks.start({
      kind: "export",
      title: `Export ${paths.length === 1 ? "1 item" : `${paths.length} items`} from ${deviceName}`,
      run: (taskId) => exportPhotos(taskId, udid, paths, destDir),
      successMessage: `Exported to ${destDir}`,
      output: destDir,
    })
  }

  const header = (
    <PageHeader
      title="Photos"
      description={
        photos.data
          ? `${items.length - videos} photos, ${videos} videos`
          : deviceName
      }
    >
      {selected.size > 0 ? (
        <Button variant="ghost" size="sm" onClick={() => setSelected(new Set())}>
          Clear
        </Button>
      ) : null}
      <Button
        variant="outline"
        size="sm"
        disabled={items.length === 0 || selected.size === items.length}
        onClick={() => setSelected(new Set(items.map((p) => p.path)))}
      >
        Select all
      </Button>
      <Button size="sm" disabled={items.length === 0} onClick={() => setExportOpen(true)}>
        <Share aria-hidden="true" />
        {selected.size ? `Export ${selected.size}` : "Export all"}
      </Button>
    </PageHeader>
  )

  return (
    <>
      {header}
      <PageBody>
        <p className="sr-only" role="status" aria-live="polite">
          {selected.size ? `${selected.size} of ${items.length} selected` : ""}
        </p>
        {photos.isPending ? (
          <ul className={GRID} aria-busy="true" aria-label="Loading photos">
            {Array.from({ length: 18 }, (_, i) => (
              <li key={i} className="overflow-hidden rounded-lg border">
                <Skeleton className="aspect-square rounded-none" />
                <div className="grid gap-1.5 border-t px-2.5 py-2">
                  <Skeleton className="h-3 w-3/4" />
                  <Skeleton className="h-2.5 w-1/2" />
                </div>
              </li>
            ))}
          </ul>
        ) : photos.isError ? (
          <EmptyState
            icon={Images}
            title="Could not read the camera roll"
            description={errorMessage(photos.error)}
            action={
              <Button variant="outline" size="sm" onClick={() => void photos.refetch()}>
                Try again
              </Button>
            }
          />
        ) : items.length === 0 ? (
          <EmptyState
            icon={Images}
            title="No photos or videos"
            description="Items in the camera roll on this device show up here."
          />
        ) : (
          <ul className={cn(GRID, "animate-in fade-in-0 duration-200")} aria-label="Camera roll">
            {items.map((photo) => (
              <PhotoTile
                key={photo.path}
                photo={photo}
                selected={selected.has(photo.path)}
                onToggle={() => toggle(photo.path)}
              />
            ))}
          </ul>
        )}
      </PageBody>
      <ExportDialog
        open={exportOpen}
        onOpenChange={setExportOpen}
        count={exportItems.length}
        bytes={exportBytes}
        onExport={runExport}
      />
    </>
  )
}

export default function PhotosScreen() {
  const { device, loading } = useDevices()
  if (!loading && device) return <PhotoLibrary key={device.udid} udid={device.udid} deviceName={device.name} />
  return (
    <>
      <PageHeader title="Photos" />
      <PageBody>
        {loading ? <Skeleton className="h-96 w-full" /> : <NoDevice />}
      </PageBody>
    </>
  )
}
