import { useQuery, useQueryClient } from "@tanstack/react-query"
import {
  BookOpen,
  Clapperboard,
  File,
  Folder,
  FolderUp,
  Music,
  RefreshCw,
  Send,
  Upload,
  type LucideIcon,
} from "lucide-react"
import { Fragment, useCallback, useMemo, useState } from "react"
import { toast } from "sonner"

import { AppIcon } from "@/components/app-icon"
import { EmptyState } from "@/components/empty-state"
import { NoDevice } from "@/components/no-device"
import { PageBody, PageHeader } from "@/components/page-header"
import { Badge } from "@/components/ui/badge"
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { useDropTarget } from "@/hooks/use-window-drop"
import {
  currentBackend,
  errorMessage,
  listMedia,
  MEDIA_ROOT,
  mediaTargets,
  pickFiles,
  pickFolder,
  sendMedia,
  type MediaCategory,
  type MediaFile,
  type MediaTarget,
  type SendSummary,
} from "@/lib/api"
import { useDevices } from "@/lib/devices"
import { formatBytes, formatRelative } from "@/lib/format"
import { useTasks } from "@/lib/tasks"
import { cn } from "@/lib/utils"

const CATEGORIES: { id: MediaCategory; label: string; icon: LucideIcon }[] = [
  { id: "audio", label: "Audio", icon: Music },
  { id: "video", label: "Video", icon: Clapperboard },
  { id: "book", label: "Books", icon: BookOpen },
  { id: "other", label: "Other", icon: File },
]

const categoryMeta = (c: MediaCategory) => CATEGORIES.find((x) => x.id === c)!

type Filter = MediaCategory | "all"

function plural(n: number, word: string) {
  return `${n} ${word}${n === 1 ? "" : "s"}`
}

/** "Sent 3 files, 1 already there, 1 renamed" for the result toast. */
function summarize(s: SendSummary): string {
  const parts = [`Sent ${plural(s.uploaded, "file")}`]
  if (s.skipped) parts.push(`${s.skipped} already there`)
  if (s.renamed.length) parts.push(`${s.renamed.length} renamed`)
  const ignored = s.skippedSymlinks + s.skippedInvalid
  if (ignored) parts.push(`${plural(ignored, "item")} ignored`)
  return parts.join(", ")
}

function TargetList({
  udid,
  targets,
  selected,
  onSelect,
}: {
  udid: string
  targets: MediaTarget[]
  selected: string | null
  onSelect: (bundleId: string) => void
}) {
  return (
    <ul className="grid gap-1" aria-label="Apps that accept files">
      {targets.map((t) => {
        const active = t.bundleId === selected
        return (
          <li key={t.bundleId}>
            <button
              type="button"
              aria-pressed={active}
              onClick={() => onSelect(t.bundleId)}
              className={cn(
                "flex w-full items-center gap-3 rounded-lg px-2.5 py-2 text-left transition-colors duration-150 outline-none hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50",
                active && "bg-muted",
              )}
            >
              <AppIcon udid={udid} bundleId={t.bundleId} name={t.name} className="size-9" />
              <span className="grid min-w-0 flex-1 gap-1">
                <span className="truncate text-sm font-medium">{t.name}</span>
                <span className="flex flex-wrap gap-1">
                  {t.recognized ? (
                    t.accepts.map((c) => (
                      <Badge key={c} variant="outline" className="bg-background px-1.5 font-normal text-muted-foreground">
                        {categoryMeta(c).label}
                      </Badge>
                    ))
                  ) : (
                    <Badge variant="outline" className="border-dashed bg-background px-1.5 font-normal text-muted-foreground">
                      Any file
                    </Badge>
                  )}
                </span>
              </span>
            </button>
          </li>
        )
      })}
    </ul>
  )
}

function FileTable({
  files,
  onOpen,
}: {
  files: MediaFile[]
  onOpen: (dir: string) => void
}) {
  return (
    <Table>
      <TableHeader className="bg-muted/40">
        <TableRow>
          <TableHead className="pl-4">Name</TableHead>
          <TableHead className="w-28">Type</TableHead>
          <TableHead className="w-24 text-right">Size</TableHead>
          <TableHead className="w-32 pr-4">Modified</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {files.map((f) => {
          const meta = f.category ? categoryMeta(f.category) : null
          const Icon = meta ? meta.icon : Folder
          return (
            <TableRow key={f.path}>
              <TableCell className="max-w-0 pl-4">
                {f.kind === "dir" ? (
                  <button
                    type="button"
                    onClick={() => onOpen(f.path)}
                    className="flex max-w-full items-center gap-2 rounded-sm font-medium outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
                  >
                    <Folder className="size-4 shrink-0 fill-muted-foreground/20 text-muted-foreground" aria-hidden="true" />
                    <span className="truncate">{f.name}</span>
                  </button>
                ) : (
                  <span className="flex items-center gap-2">
                    <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                    <span className="truncate" title={f.name}>
                      {f.name}
                    </span>
                  </span>
                )}
              </TableCell>
              <TableCell className="text-muted-foreground">{meta ? meta.label : "Folder"}</TableCell>
              <TableCell className="text-right tabular-nums">{f.kind === "dir" ? "-" : formatBytes(f.size)}</TableCell>
              <TableCell className="pr-4 text-muted-foreground">{formatRelative(f.modified)}</TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}

function TargetContents({ udid, target }: { udid: string; target: MediaTarget }) {
  const [dir, setDir] = useState(MEDIA_ROOT)
  const [filter, setFilter] = useState<Filter>("all")
  const tasks = useTasks()
  const queryClient = useQueryClient()
  const key = ["media", udid, target.bundleId, dir]
  const files = useQuery({ queryKey: key, queryFn: () => listMedia(udid, target.bundleId, dir) })

  const subdir = dir === MEDIA_ROOT ? null : dir.slice(MEDIA_ROOT.length + 1)
  const segments = subdir ? subdir.split("/") : []

  const counts = useMemo(() => {
    const c: Record<Filter, number> = { all: 0, audio: 0, video: 0, book: 0, other: 0 }
    for (const f of files.data ?? []) {
      if (!f.category) continue
      c.all++
      c[f.category]++
    }
    return c
  }, [files.data])

  const visible = useMemo(() => {
    const list = (files.data ?? []).filter((f) => filter === "all" || f.category === filter || f.kind === "dir")
    return list.sort((a, b) =>
      a.kind === "dir" && b.kind !== "dir" ? -1 : a.kind !== "dir" && b.kind === "dir" ? 1 : a.name.localeCompare(b.name),
    )
  }, [files.data, filter])

  const send = useCallback(
    (paths: string[]) => {
    if (paths.length === 0) return
    void tasks
      .start({
        kind: "transfer",
        title: `Send to ${target.name}`,
        successMessage: false,
        run: (taskId) =>
          sendMedia(taskId, udid, target.bundleId, paths, dir === MEDIA_ROOT ? null : dir.slice(MEDIA_ROOT.length + 1)),
      })
      .then((summary) => {
        if (!summary) return
        toast.success(summarize(summary), {
          description: summary.renamed.length
            ? `Renamed: ${summary.renamed.map((p) => p.split("/").pop()).join(", ")}`
            : undefined,
        })
        void queryClient.invalidateQueries({ queryKey: ["media", udid, target.bundleId] })
      })
    },
    [tasks, udid, target, dir, queryClient],
  )

  const where = dir === MEDIA_ROOT ? target.name : dir.slice(dir.lastIndexOf("/") + 1)
  const dropTarget = useMemo(
    () => ({
      label: `Drop to send to ${where}`,
      handler: (paths: string[]) => {
        send(paths)
        return true
      },
    }),
    [where, send],
  )
  useDropTarget(dropTarget)

  return (
    <section className="flex min-w-0 flex-col gap-3" aria-label={`${target.name} files`}>
      <div className="flex flex-wrap items-center gap-2">
        <Breadcrumb className="min-w-0 flex-1">
          <BreadcrumbList>
            <BreadcrumbItem>
              {segments.length ? (
                <button
                  type="button"
                  onClick={() => setDir(MEDIA_ROOT)}
                  className="rounded-sm outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
                >
                  {target.name}
                </button>
              ) : (
                <BreadcrumbPage>{target.name}</BreadcrumbPage>
              )}
            </BreadcrumbItem>
            {segments.map((seg, i) => {
              const path = `${MEDIA_ROOT}/${segments.slice(0, i + 1).join("/")}`
              const last = i === segments.length - 1
              return (
                <Fragment key={path}>
                  <BreadcrumbSeparator />
                  <BreadcrumbItem>
                    {last ? (
                      <BreadcrumbPage>{seg}</BreadcrumbPage>
                    ) : (
                      <button
                        type="button"
                        onClick={() => setDir(path)}
                        className="rounded-sm outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
                      >
                        {seg}
                      </button>
                    )}
                  </BreadcrumbItem>
                </Fragment>
              )
            })}
          </BreadcrumbList>
        </Breadcrumb>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Refresh"
          onClick={() => void files.refetch()}
          disabled={files.isFetching}
        >
          <RefreshCw className={cn(files.isFetching && "animate-spin motion-reduce:animate-none")} />
        </Button>
        {/* A remote client can only upload files, not folders. */}
        {currentBackend() === "remote" ? null : (
          <Button
            variant="outline"
            size="sm"
            onClick={() => void pickFolder("Send a folder").then((p) => p && send([p]))}
          >
            <FolderUp aria-hidden="true" />
            Send folder...
          </Button>
        )}
        <Button size="sm" onClick={() => void pickFiles("Send files").then(send)}>
          <Upload aria-hidden="true" />
          Send files...
        </Button>
      </div>

      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        spacing={0}
        value={filter}
        onValueChange={(v) => v && setFilter(v as Filter)}
        aria-label="Filter by type"
        className="self-start"
      >
        <ToggleGroupItem value="all" className="px-3">
          All <span className="text-muted-foreground tabular-nums">{counts.all}</span>
        </ToggleGroupItem>
        {CATEGORIES.map((c) => (
          <ToggleGroupItem key={c.id} value={c.id} className="px-3">
            {c.label} <span className="text-muted-foreground tabular-nums">{counts[c.id]}</span>
          </ToggleGroupItem>
        ))}
      </ToggleGroup>

      <div className="overflow-hidden rounded-lg border">
        {files.isPending ? (
          <div className="grid gap-3 p-4" aria-busy="true" aria-label="Loading files">
            {Array.from({ length: 5 }, (_, i) => (
              <Skeleton key={i} className="h-6 w-full" />
            ))}
          </div>
        ) : files.isError ? (
          <EmptyState
            icon={File}
            title="Could not list files"
            description={errorMessage(files.error)}
            className="m-4"
            action={
              <Button variant="outline" size="sm" onClick={() => void files.refetch()}>
                Try again
              </Button>
            }
          />
        ) : visible.length === 0 ? (
          <EmptyState
            icon={Send}
            title={files.data.length ? "Nothing of this type" : "Nothing here yet"}
            description={
              files.data.length
                ? "Pick another type above."
                : `Drop files or folders anywhere on the window to send them to ${target.name}.`
            }
            className="m-4 border-none"
          />
        ) : (
          <FileTable files={visible} onOpen={setDir} />
        )}
      </div>
      <p className="text-xs text-muted-foreground">
        Files with the same name and size are skipped. A different file with the same name is kept and the new one gets a
        numbered name.
      </p>
    </section>
  )
}

function MediaView({ udid }: { udid: string }) {
  const targets = useQuery({ queryKey: ["mediaTargets", udid], queryFn: () => mediaTargets(udid) })
  const [picked, setPicked] = useState<string | null>(null)
  const list = targets.data ?? []
  const selected = list.find((t) => t.bundleId === picked) ?? list[0] ?? null

  if (targets.isPending) {
    return (
      <div className="grid gap-4 lg:grid-cols-[17rem_1fr]" aria-busy="true" aria-label="Loading apps">
        <div className="grid content-start gap-2">
          {Array.from({ length: 3 }, (_, i) => (
            <Skeleton key={i} className="h-13 w-full" />
          ))}
        </div>
        <Skeleton className="h-80 w-full" />
      </div>
    )
  }
  if (targets.isError) {
    return (
      <EmptyState
        icon={Clapperboard}
        title="Could not list apps"
        description={errorMessage(targets.error)}
        action={
          <Button variant="outline" size="sm" onClick={() => void targets.refetch()}>
            Try again
          </Button>
        }
      />
    )
  }
  if (!selected) {
    return (
      <EmptyState
        icon={Clapperboard}
        title="No apps accept files"
        description="Apps like VLC or Infuse accept media through file sharing. Install one, then send files to it here."
      />
    )
  }
  return (
    <div className="grid gap-6 lg:grid-cols-[17rem_1fr] animate-in fade-in-0 duration-200">
      <div className="grid content-start gap-2">
        <h2 className="px-2.5 text-xs font-medium text-muted-foreground">Send to</h2>
        <TargetList udid={udid} targets={list} selected={selected.bundleId} onSelect={setPicked} />
      </div>
      <TargetContents key={selected.bundleId} udid={udid} target={selected} />
    </div>
  )
}

export default function MediaScreen() {
  const { device, loading } = useDevices()
  return (
    <>
      <PageHeader title="Media" description="Send music, videos and books to apps" />
      <PageBody>
        {loading ? (
          <Skeleton className="h-80 w-full" />
        ) : device ? (
          <MediaView key={device.udid} udid={device.udid} />
        ) : (
          <NoDevice />
        )}
      </PageBody>
    </>
  )
}
