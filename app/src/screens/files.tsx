import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import {
  Download,
  File,
  FileArchive,
  FileAudio,
  FileImage,
  FileText,
  FileVideo,
  Folder,
  FolderOpen,
  FolderPlus,
  HardDrive,
  Link,
  MoreHorizontal,
  Pencil,
  RefreshCw,
  Trash2,
  Upload,
} from "lucide-react"
import { Fragment, useCallback, useMemo, useState } from "react"
import { toast } from "sonner"

import { AppIcon } from "@/components/app-icon"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { EmptyState } from "@/components/empty-state"
import { downloadDestination } from "@/components/files/destination"
import { NameDialog } from "@/components/files/name-dialog"
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
import { Checkbox } from "@/components/ui/checkbox"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { useDropTarget } from "@/hooks/use-window-drop"
import {
  createFolder,
  deletePaths,
  downloadFiles,
  errorMessage,
  listApps,
  listFiles,
  pickFiles,
  renamePath,
  uploadFiles,
  type FileEntry,
  type FileSource,
  type InstalledApp,
} from "@/lib/api"
import { useDevices } from "@/lib/devices"
import { formatBytes, formatDateTime, formatRelative } from "@/lib/format"
import { useTasks } from "@/lib/tasks"
import { cn } from "@/lib/utils"

type SourceTab = "media" | "apps"
type ContainerMode = "appDocuments" | "appContainer"

function joinPath(dir: string, name: string) {
  return dir === "/" ? `/${name}` : `${dir}/${name}`
}

function fileIcon(entry: FileEntry) {
  if (entry.kind === "dir") return Folder
  if (entry.kind === "symlink") return Link
  const ext = entry.name.split(".").pop()?.toLowerCase() ?? ""
  if (["heic", "jpg", "jpeg", "png", "gif", "webp", "tiff"].includes(ext)) return FileImage
  if (["mov", "mp4", "m4v"].includes(ext)) return FileVideo
  if (["m4a", "mp3", "wav", "aac", "caf"].includes(ext)) return FileAudio
  if (["zip", "ipa", "gz", "tar"].includes(ext)) return FileArchive
  if (["txt", "md", "pdf", "csv", "json", "plist", "xlsx", "docx"].includes(ext)) return FileText
  return File
}

function sortEntries(entries: FileEntry[]) {
  return [...entries].sort((a, b) => {
    const ad = a.kind === "dir" ? 0 : 1
    const bd = b.kind === "dir" ? 0 : 1
    return ad - bd || a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" })
  })
}

function SkeletonRows() {
  return (
    <>
      {Array.from({ length: 8 }, (_, i) => (
        <TableRow key={i} className="hover:bg-transparent">
          <TableCell className="w-10">
            <Skeleton className="size-4 rounded-[4px]" />
          </TableCell>
          <TableCell>
            <div className="flex items-center gap-2">
              <Skeleton className="size-4" />
              <Skeleton className="h-4" style={{ width: `${30 + ((i * 17) % 40)}%` }} />
            </div>
          </TableCell>
          <TableCell>
            <Skeleton className="ml-auto h-4 w-14" />
          </TableCell>
          <TableCell className="hidden md:table-cell">
            <Skeleton className="h-4 w-24" />
          </TableCell>
          <TableCell />
        </TableRow>
      ))}
    </>
  )
}

function AppPicker({
  apps,
  loading,
  value,
  onChange,
  udid,
}: {
  apps: InstalledApp[]
  loading: boolean
  value: string | null
  onChange: (bundleId: string) => void
  udid: string
}) {
  if (loading) return <Skeleton className="h-8 w-56" />
  return (
    <Select value={value ?? undefined} onValueChange={onChange} disabled={apps.length === 0}>
      <SelectTrigger className="w-64" aria-label="App">
        <SelectValue placeholder={apps.length ? "Choose an app" : "No user apps"} />
      </SelectTrigger>
      <SelectContent className="max-h-80">
        {apps.map((app) => (
          <SelectItem key={app.bundleId} value={app.bundleId}>
            <AppIcon udid={udid} bundleId={app.bundleId} name={app.name} className="size-4 text-[8px]" />
            <span className="truncate">{app.name}</span>
            {app.fileSharing ? (
              <span className="ml-1 text-xs text-muted-foreground">File sharing</span>
            ) : null}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

function FileBrowser({ udid }: { udid: string }) {
  const queryClient = useQueryClient()
  const tasks = useTasks()
  const [tab, setTab] = useState<SourceTab>("media")
  const [bundleId, setBundleId] = useState<string | null>(null)
  const [mode, setMode] = useState<ContainerMode>("appDocuments")
  const [path, setPath] = useState("/")
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [newFolderOpen, setNewFolderOpen] = useState(false)
  const [renaming, setRenaming] = useState<FileEntry | null>(null)
  const [deleting, setDeleting] = useState<string[] | null>(null)

  const appsQuery = useQuery({
    queryKey: ["apps", udid],
    queryFn: () => listApps(udid),
    enabled: tab === "apps",
  })
  const userApps = useMemo(
    () =>
      (appsQuery.data ?? [])
        .filter((a) => a.kind === "user")
        .sort((a, b) => Number(b.fileSharing) - Number(a.fileSharing) || a.name.localeCompare(b.name)),
    [appsQuery.data],
  )
  const activeBundle = bundleId ?? userApps[0]?.bundleId ?? null
  const activeApp = userApps.find((a) => a.bundleId === activeBundle) ?? null

  const source = useMemo<FileSource | null>(() => {
    if (tab === "media") return { kind: "media" }
    return activeBundle ? { kind: mode, bundleId: activeBundle } : null
  }, [tab, mode, activeBundle])

  const rootLabel = tab === "media" ? "Media" : activeApp ? activeApp.name : "App"
  const filesKey = ["files", udid, source, path] as const
  const files = useQuery({
    queryKey: filesKey,
    queryFn: () => listFiles(udid, source!, path),
    enabled: !!source,
  })
  const entries = useMemo(() => sortEntries(files.data ?? []), [files.data])

  const go = (next: string) => {
    setPath(next)
    setSelected(new Set())
  }
  const resetLocation = () => go("/")

  const refresh = () => queryClient.invalidateQueries({ queryKey: ["files", udid, source, path] })
  const folderName = path === "/" ? rootLabel : path.slice(path.lastIndexOf("/") + 1)

  const { start } = tasks
  const upload = useCallback(
    (localPaths: string[]) => {
      if (!source || localPaths.length === 0) return
      const dest = path
      const src = source
      const n = localPaths.length
      void start({
        kind: "transfer",
        title: n === 1 ? `Upload ${localPaths[0].split(/[\\/]/).pop()}` : `Upload ${n} files`,
        successMessage: `Uploaded ${n === 1 ? "1 file" : `${n} files`} to ${folderName}`,
        run: (taskId) => uploadFiles(taskId, udid, src, dest, localPaths),
      }).then(() => queryClient.invalidateQueries({ queryKey: ["files", udid, src, dest] }))
    },
    [source, path, folderName, udid, start, queryClient],
  )

  const download = async (remotePaths: string[]) => {
    if (!source || remotePaths.length === 0) return
    const dest = await downloadDestination("Download to")
    if (!dest) return
    const src = source
    const n = remotePaths.length
    void tasks.start({
      kind: "transfer",
      title: n === 1 ? `Download ${remotePaths[0].split("/").pop()}` : `Download ${n} items`,
      successMessage: `Downloaded to ${dest}`,
      output: dest,
      run: (taskId) => downloadFiles(taskId, udid, src, remotePaths, dest),
    })
  }

  const dropTarget = useMemo(
    () =>
      source
        ? {
            label: `Drop to upload to ${folderName}`,
            handler: (paths: string[]) => {
              // IPAs alone fall through to the install sheet.
              if (paths.every((p) => p.toLowerCase().endsWith(".ipa"))) return false
              upload(paths)
              return true
            },
          }
        : null,
    [source, folderName, upload],
  )
  useDropTarget(dropTarget)

  const mkdir = useMutation({
    mutationFn: (name: string) => createFolder(udid, source!, joinPath(path, name)),
    onSuccess: (_, name) => {
      toast.success(`Created ${name}`)
      setNewFolderOpen(false)
      void refresh()
    },
  })
  const rename = useMutation({
    mutationFn: ({ entry, name }: { entry: FileEntry; name: string }) =>
      renamePath(udid, source!, entry.path, joinPath(path, name)),
    onSuccess: (_, { name }) => {
      toast.success(`Renamed to ${name}`)
      setRenaming(null)
      setSelected(new Set())
      void refresh()
    },
  })
  const remove = useMutation({
    mutationFn: (paths: string[]) => deletePaths(udid, source!, paths),
    onSuccess: (_, paths) => {
      toast.success(paths.length === 1 ? "Deleted 1 item" : `Deleted ${paths.length} items`)
      setSelected(new Set())
      void refresh()
    },
    onError: (e) => toast.error("Delete failed", { description: errorMessage(e) }),
  })

  const toggle = (p: string, on?: boolean) =>
    setSelected((prev) => {
      const next = new Set(prev)
      if (on ?? !next.has(p)) next.add(p)
      else next.delete(p)
      return next
    })
  const allSelected = entries.length > 0 && selected.size === entries.length
  const someSelected = selected.size > 0 && !allSelected
  const selectedPaths = entries.filter((e) => selected.has(e.path)).map((e) => e.path)

  const segments = path === "/" ? [] : path.slice(1).split("/")
  const writable = !!source

  return (
    <div className="flex flex-1 flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          spacing={0}
          value={tab}
          onValueChange={(v) => {
            if (!v) return
            setTab(v as SourceTab)
            resetLocation()
          }}
          aria-label="Source"
        >
          <ToggleGroupItem value="media" className="px-3">
            <HardDrive aria-hidden="true" />
            Media
          </ToggleGroupItem>
          <ToggleGroupItem value="apps" className="px-3">
            <FolderOpen aria-hidden="true" />
            App containers
          </ToggleGroupItem>
        </ToggleGroup>
        {tab === "apps" ? (
          <>
            <AppPicker
              apps={userApps}
              loading={appsQuery.isPending}
              value={activeBundle}
              udid={udid}
              onChange={(b) => {
                setBundleId(b)
                resetLocation()
              }}
            />
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              spacing={0}
              value={mode}
              onValueChange={(v) => {
                if (!v) return
                setMode(v as ContainerMode)
                resetLocation()
              }}
              aria-label="Container scope"
            >
              <ToggleGroupItem value="appDocuments" className="px-3">
                Documents
              </ToggleGroupItem>
              <ToggleGroupItem value="appContainer" className="px-3">
                Full container
              </ToggleGroupItem>
            </ToggleGroup>
            {activeApp && mode === "appDocuments" && !activeApp.fileSharing ? (
              <Badge variant="outline" className="font-normal text-muted-foreground">
                No file sharing, Documents may be unavailable
              </Badge>
            ) : null}
          </>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Breadcrumb className="min-w-0 flex-1">
          <BreadcrumbList className="flex-nowrap">
            <BreadcrumbItem className="shrink-0">
              {segments.length === 0 ? (
                <BreadcrumbPage className="font-medium">{rootLabel}</BreadcrumbPage>
              ) : (
                <Button variant="ghost" size="xs" className="-mx-2 text-muted-foreground" onClick={() => go("/")}>
                  {rootLabel}
                </Button>
              )}
            </BreadcrumbItem>
            {segments.map((seg, i) => {
              const target = `/${segments.slice(0, i + 1).join("/")}`
              const last = i === segments.length - 1
              return (
                <Fragment key={target}>
                  <BreadcrumbSeparator />
                  <BreadcrumbItem className="min-w-0">
                    {last ? (
                      <BreadcrumbPage className="truncate font-medium">{seg}</BreadcrumbPage>
                    ) : (
                      <Button
                        variant="ghost"
                        size="xs"
                        className="-mx-2 max-w-40 truncate text-muted-foreground"
                        onClick={() => go(target)}
                      >
                        {seg}
                      </Button>
                    )}
                  </BreadcrumbItem>
                </Fragment>
              )
            })}
          </BreadcrumbList>
        </Breadcrumb>
        <div className="flex items-center gap-2">
          {selected.size > 0 ? (
            <>
              <span className="text-sm text-muted-foreground tabular-nums" aria-live="polite">
                {selected.size} selected
              </span>
              <Button variant="outline" size="sm" onClick={() => void download(selectedPaths)}>
                <Download aria-hidden="true" />
                Download
              </Button>
              <Button variant="destructive" size="sm" onClick={() => setDeleting(selectedPaths)}>
                <Trash2 aria-hidden="true" />
                Delete
              </Button>
            </>
          ) : null}
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Refresh"
            disabled={!writable}
            onClick={() => void refresh()}
          >
            <RefreshCw className={cn(files.isFetching && "animate-spin motion-reduce:animate-none")} />
          </Button>
          <Button variant="outline" size="sm" disabled={!writable} onClick={() => setNewFolderOpen(true)}>
            <FolderPlus aria-hidden="true" />
            New folder
          </Button>
          <Button
            size="sm"
            disabled={!writable}
            onClick={() => void pickFiles("Choose files to upload").then(upload)}
          >
            <Upload aria-hidden="true" />
            Upload
          </Button>
        </div>
      </div>

      {tab === "apps" && !appsQuery.isPending && userApps.length === 0 ? (
        <EmptyState
          icon={FolderOpen}
          title="No user apps"
          description="Install an app to browse its container."
        />
      ) : files.isError ? (
        <EmptyState
          icon={FolderOpen}
          title="Could not open this folder"
          description={errorMessage(files.error)}
          action={
            <Button variant="outline" size="sm" onClick={() => void refresh()}>
              Try again
            </Button>
          }
        />
      ) : !files.isPending && entries.length === 0 ? (
        <EmptyState
          icon={FolderOpen}
          title="This folder is empty"
          description="Upload files with the button above or drop them on the window."
        />
      ) : (
        <div className="overflow-hidden rounded-lg border">
          <Table aria-label={`Contents of ${folderName}`} aria-busy={files.isPending || undefined}>
            <TableHeader className="bg-muted/40">
              <TableRow className="hover:bg-transparent">
                <TableHead className="w-10 pl-3">
                  <Checkbox
                    aria-label="Select all"
                    className="data-[state=indeterminate]:border-primary data-[state=indeterminate]:bg-primary data-[state=indeterminate]:text-primary-foreground data-[state=indeterminate]:[&_svg]:hidden data-[state=indeterminate]:before:h-0.5 data-[state=indeterminate]:before:w-2 data-[state=indeterminate]:before:rounded-full data-[state=indeterminate]:before:bg-current"
                    disabled={entries.length === 0}
                    checked={allSelected ? true : someSelected ? "indeterminate" : false}
                    onCheckedChange={(v) => setSelected(v === true ? new Set(entries.map((e) => e.path)) : new Set())}
                  />
                </TableHead>
                <TableHead>Name</TableHead>
                <TableHead className="w-28 text-right">Size</TableHead>
                <TableHead className="hidden w-44 md:table-cell">Modified</TableHead>
                <TableHead className="w-12">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {files.isPending ? (
                <SkeletonRows />
              ) : (
                entries.map((entry) => {
                  const Icon = fileIcon(entry)
                  const isDir = entry.kind === "dir"
                  const checked = selected.has(entry.path)
                  return (
                    <TableRow
                      key={entry.path}
                      tabIndex={0}
                      data-state={checked ? "selected" : undefined}
                      aria-selected={checked}
                      aria-label={isDir ? `Folder ${entry.name}` : entry.name}
                      className="cursor-default outline-none focus-visible:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:ring-inset"
                      onClick={(e) => {
                        if ((e.target as HTMLElement).closest("button,[role=checkbox],[role=menuitem]")) return
                        if (e.metaKey || e.ctrlKey) toggle(entry.path)
                        else setSelected(new Set([entry.path]))
                      }}
                      onDoubleClick={() => isDir && go(entry.path)}
                      onKeyDown={(e) => {
                        if (e.target !== e.currentTarget) return
                        if (e.key === "Enter" && isDir) {
                          e.preventDefault()
                          go(entry.path)
                        } else if (e.key === " ") {
                          e.preventDefault()
                          toggle(entry.path)
                        } else if (e.key === "Delete") {
                          e.preventDefault()
                          setDeleting([entry.path])
                        }
                      }}
                    >
                      <TableCell className="pl-3">
                        <Checkbox
                          aria-label={`Select ${entry.name}`}
                          checked={checked}
                          onCheckedChange={(v) => toggle(entry.path, v === true)}
                          tabIndex={-1}
                        />
                      </TableCell>
                      <TableCell className="max-w-0">
                        <div className="flex min-w-0 items-center gap-2">
                          <Icon
                            className={cn(
                              "size-4 shrink-0",
                              isDir ? "fill-muted-foreground/20 text-muted-foreground" : "text-muted-foreground",
                            )}
                            aria-hidden="true"
                          />
                          {isDir ? (
                            <button
                              type="button"
                              tabIndex={-1}
                              className="truncate text-left font-medium hover:underline"
                              title={entry.name}
                              onClick={() => go(entry.path)}
                            >
                              {entry.name}
                            </button>
                          ) : (
                            <span className="truncate" title={entry.name}>
                              {entry.name}
                            </span>
                          )}
                        </div>
                      </TableCell>
                      <TableCell className="text-right text-muted-foreground tabular-nums">
                        {isDir ? "-" : formatBytes(entry.size)}
                      </TableCell>
                      <TableCell
                        className="hidden text-muted-foreground md:table-cell"
                        title={formatDateTime(entry.modified)}
                      >
                        {formatRelative(entry.modified)}
                      </TableCell>
                      <TableCell className="pr-3 text-right">
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button variant="ghost" size="icon-xs" aria-label={`Actions for ${entry.name}`}>
                              <MoreHorizontal />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end" className="w-40">
                            {isDir ? (
                              <DropdownMenuItem onSelect={() => go(entry.path)}>
                                <FolderOpen aria-hidden="true" />
                                Open
                              </DropdownMenuItem>
                            ) : null}
                            <DropdownMenuItem onSelect={() => void download([entry.path])}>
                              <Download aria-hidden="true" />
                              Download
                            </DropdownMenuItem>
                            <DropdownMenuItem
                              onSelect={() => {
                                rename.reset()
                                setRenaming(entry)
                              }}
                            >
                              <Pencil aria-hidden="true" />
                              Rename
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem variant="destructive" onSelect={() => setDeleting([entry.path])}>
                              <Trash2 aria-hidden="true" />
                              Delete
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </TableCell>
                    </TableRow>
                  )
                })
              )}
            </TableBody>
          </Table>
        </div>
      )}

      <NameDialog
        open={newFolderOpen}
        onOpenChange={(o) => {
          setNewFolderOpen(o)
          if (!o) mkdir.reset()
        }}
        title="New folder"
        description={`Create a folder in ${folderName}.`}
        label="Folder name"
        initial=""
        submitLabel="Create"
        pending={mkdir.isPending}
        error={mkdir.isError ? errorMessage(mkdir.error) : null}
        onSubmit={(name) => mkdir.mutate(name)}
      />
      <NameDialog
        open={!!renaming}
        onOpenChange={(o) => !o && setRenaming(null)}
        title="Rename"
        description={renaming ? `Rename ${renaming.name}.` : ""}
        label="New name"
        initial={renaming?.name ?? ""}
        submitLabel="Rename"
        pending={rename.isPending}
        error={rename.isError ? errorMessage(rename.error) : null}
        onSubmit={(name) => renaming && rename.mutate({ entry: renaming, name })}
      />
      <ConfirmDialog
        open={!!deleting}
        onOpenChange={(o) => !o && setDeleting(null)}
        title={
          deleting?.length === 1
            ? `Delete ${deleting[0].split("/").pop()}?`
            : `Delete ${deleting?.length ?? 0} items?`
        }
        description="Deleted items are removed from the device immediately and cannot be recovered. Folders are deleted with their contents."
        actionLabel="Delete"
        onConfirm={() => {
          if (deleting) remove.mutate(deleting)
          setDeleting(null)
        }}
      >
        {deleting && deleting.length > 1 ? (
          <ul className="max-h-32 overflow-y-auto rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs text-muted-foreground">
            {deleting.map((p) => (
              <li key={p} className="truncate">
                {p}
              </li>
            ))}
          </ul>
        ) : null}
      </ConfirmDialog>
    </div>
  )
}

export default function FilesScreen() {
  const { device, loading } = useDevices()
  return (
    <>
      <PageHeader title="Files" description={device ? device.name : undefined} />
      <PageBody>
        {loading ? (
          <Skeleton className="h-96 w-full" />
        ) : device ? (
          <FileBrowser key={device.udid} udid={device.udid} />
        ) : (
          <NoDevice />
        )}
      </PageBody>
    </>
  )
}
