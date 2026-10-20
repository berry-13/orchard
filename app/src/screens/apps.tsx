import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { LayoutGrid, MoreHorizontal, PackagePlus, PenLine, Search, SearchX, Trash2 } from "lucide-react"
import { useDeferredValue, useMemo, useState } from "react"
import { toast } from "sonner"

import { AppIcon } from "@/components/app-icon"
import { ConfirmDialog } from "@/components/confirm-dialog"
import { EmptyState } from "@/components/empty-state"
import { useInstallIpa } from "@/components/ipa-install"
import { NoDevice } from "@/components/no-device"
import { PageBody, PageHeader } from "@/components/page-header"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { errorMessage, listApps, uninstallApp, type InstalledApp } from "@/lib/api"
import { useDevices } from "@/lib/devices"
import { formatBytes } from "@/lib/format"

type Filter = "user" | "system" | "all"

function SkeletonRows() {
  return Array.from({ length: 8 }, (_, i) => (
    <TableRow key={i} className="hover:bg-transparent">
      <TableCell>
        <div className="flex items-center gap-3">
          <Skeleton className="size-8 rounded-[22%]" />
          <div className="grid gap-1.5">
            <Skeleton className="h-3.5 w-32" />
            <Skeleton className="h-3 w-44" />
          </div>
        </div>
      </TableCell>
      <TableCell>
        <Skeleton className="h-3.5 w-14" />
      </TableCell>
      <TableCell>
        <Skeleton className="ml-auto h-3.5 w-14" />
      </TableCell>
      <TableCell>
        <Skeleton className="h-5 w-12 rounded-full" />
      </TableCell>
      <TableCell />
    </TableRow>
  ))
}

function AppsTable({ udid, deviceName }: { udid: string; deviceName: string }) {
  const queryClient = useQueryClient()
  const installIpa = useInstallIpa()
  const resignIpa = useInstallIpa("resign")
  const [filter, setFilter] = useState<Filter>("user")
  const [search, setSearch] = useState("")
  const query = useDeferredValue(search.trim().toLowerCase())
  const [pending, setPending] = useState<InstalledApp | null>(null)

  const apps = useQuery({ queryKey: ["apps", udid], queryFn: () => listApps(udid) })

  const uninstall = useMutation({
    mutationFn: (app: InstalledApp) => uninstallApp(udid, app.bundleId),
    onMutate: (app) => {
      const id = toast.loading(`Removing ${app.name}...`)
      return { id }
    },
    onSuccess: (_, app, ctx) => {
      toast.success(`${app.name} removed from ${deviceName}`, { id: ctx?.id })
      void queryClient.invalidateQueries({ queryKey: ["apps", udid] })
    },
    onError: (e, app, ctx) => toast.error(`Could not remove ${app.name}`, { id: ctx?.id, description: errorMessage(e) }),
  })

  const counts = useMemo(() => {
    const list = apps.data ?? []
    const user = list.filter((a) => a.kind === "user").length
    return { user, system: list.length - user, all: list.length }
  }, [apps.data])

  const rows = useMemo(() => {
    return (apps.data ?? [])
      .filter((a) => filter === "all" || a.kind === filter)
      .filter(
        (a) => !query || a.name.toLowerCase().includes(query) || a.bundleId.toLowerCase().includes(query),
      )
      .sort((a, b) => a.name.localeCompare(b.name))
  }, [apps.data, filter, query])

  return (
    <>
      <PageHeader
        title="Apps"
        description={apps.data ? `${counts.user} user apps, ${counts.system} system apps` : undefined}
      >
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="outline" size="icon-sm" aria-label="More actions">
              <MoreHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuItem onSelect={() => void resignIpa()}>
              <PenLine aria-hidden="true" />
              Re-sign IPA...
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button size="sm" onClick={() => void installIpa()}>
          <PackagePlus aria-hidden="true" />
          Install IPA
        </Button>
      </PageHeader>
      <PageBody>
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <InputGroup className="sm:max-w-xs">
            <InputGroupAddon>
              <Search aria-hidden="true" />
            </InputGroupAddon>
            <InputGroupInput
              type="search"
              placeholder="Search apps or bundle IDs"
              aria-label="Search apps"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </InputGroup>
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            spacing={0}
            value={filter}
            onValueChange={(v) => v && setFilter(v as Filter)}
            aria-label="Filter apps by kind"
          >
            <ToggleGroupItem value="user" className="px-3">
              User <span className="text-muted-foreground tabular-nums">{counts.user}</span>
            </ToggleGroupItem>
            <ToggleGroupItem value="system" className="px-3">
              System <span className="text-muted-foreground tabular-nums">{counts.system}</span>
            </ToggleGroupItem>
            <ToggleGroupItem value="all" className="px-3">
              All <span className="text-muted-foreground tabular-nums">{counts.all}</span>
            </ToggleGroupItem>
          </ToggleGroup>
        </div>

        {apps.isError ? (
          <EmptyState
            icon={LayoutGrid}
            title="Could not load apps"
            description={errorMessage(apps.error)}
            action={
              <Button variant="outline" size="sm" onClick={() => void apps.refetch()}>
                Try again
              </Button>
            }
          />
        ) : !apps.isPending && rows.length === 0 ? (
          <EmptyState
            icon={query ? SearchX : LayoutGrid}
            title={query ? "No matching apps" : "No apps here"}
            description={
              query ? `Nothing matches "${search.trim()}". Try a name or a bundle ID.` : "Install an IPA to get started."
            }
          />
        ) : (
          <div className="overflow-hidden rounded-lg border">
            <Table aria-busy={apps.isPending || undefined}>
              <TableHeader className="bg-muted/40">
                <TableRow className="hover:bg-transparent">
                  <TableHead className="pl-3">Name</TableHead>
                  <TableHead className="w-32">Version</TableHead>
                  <TableHead className="w-28 text-right">Size</TableHead>
                  <TableHead className="w-24">Kind</TableHead>
                  <TableHead className="w-12">
                    <span className="sr-only">Actions</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {apps.isPending ? (
                  <SkeletonRows />
                ) : (
                  rows.map((app) => (
                    <TableRow key={app.bundleId}>
                      <TableCell className="max-w-0 pl-3">
                        <div className="flex min-w-0 items-center gap-3">
                          <AppIcon udid={udid} bundleId={app.bundleId} name={app.name} />
                          <div className="grid min-w-0">
                            <span className="truncate font-medium">{app.name}</span>
                            <span className="truncate font-mono text-xs text-muted-foreground">
                              {app.bundleId}
                            </span>
                          </div>
                        </div>
                      </TableCell>
                      <TableCell className="tabular-nums">
                        <span className="block truncate" title={`${app.shortVersion} (${app.version})`}>
                          {app.shortVersion}
                        </span>
                      </TableCell>
                      <TableCell className="text-right text-muted-foreground tabular-nums">
                        {formatBytes(app.size)}
                      </TableCell>
                      <TableCell>
                        <Badge variant={app.kind === "user" ? "secondary" : "outline"} className="font-normal">
                          {app.kind === "user" ? "User" : "System"}
                        </Badge>
                      </TableCell>
                      <TableCell className="pr-3 text-right">
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${app.name}`}>
                              <MoreHorizontal />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end" className="w-48">
                            <DropdownMenuLabel className="truncate text-xs text-muted-foreground">
                              {app.name}
                            </DropdownMenuLabel>
                            <DropdownMenuItem
                              onSelect={() => {
                                void navigator.clipboard.writeText(app.bundleId)
                                toast.success("Bundle ID copied")
                              }}
                            >
                              Copy bundle ID
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              variant="destructive"
                              disabled={app.kind === "system"}
                              onSelect={() => setPending(app)}
                            >
                              <Trash2 aria-hidden="true" />
                              Uninstall
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </div>
        )}
        <p className="sr-only" role="status" aria-live="polite">
          {apps.isPending ? "Loading apps" : `${rows.length} apps shown`}
        </p>
      </PageBody>

      <ConfirmDialog
        open={pending !== null}
        onOpenChange={(open) => !open && setPending(null)}
        title={`Uninstall ${pending?.name ?? "app"}?`}
        description={`The app and all of its data are removed from ${deviceName}. This cannot be undone.`}
        actionLabel="Uninstall"
        onConfirm={() => {
          if (pending) uninstall.mutate(pending)
          setPending(null)
        }}
      />
    </>
  )
}

export default function AppsScreen() {
  const { device, loading } = useDevices()
  if (device) return <AppsTable key={device.udid} udid={device.udid} deviceName={device.name} />
  return (
    <>
      <PageHeader title="Apps" />
      <PageBody>{loading ? <Skeleton className="h-96" /> : <NoDevice />}</PageBody>
    </>
  )
}
