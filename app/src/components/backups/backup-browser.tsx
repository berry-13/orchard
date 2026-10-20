import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import {
  ArrowLeft,
  ChevronRight,
  Download,
  FileSearch,
  FolderTree,
  KeyRound,
  LayoutGrid,
  Lock,
  RotateCcw,
  Search,
  Server,
} from "lucide-react"
import { useDeferredValue, useState } from "react"

import { ConfirmDialog } from "@/components/confirm-dialog"
import { EmptyState } from "@/components/empty-state"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group"
import { Label } from "@/components/ui/label"
import { Skeleton } from "@/components/ui/skeleton"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import {
  errorMessage,
  extractBackupFiles,
  getSettings,
  isOrchardError,
  listBackupFiles,
  openBackup,
  pickFolder,
  restoreBackup,
  type BackupDomain,
  type BackupSummary,
  type Device,
} from "@/lib/api"
import { formatBytes, formatDate, formatDateTime, formatNumber } from "@/lib/format"
import { useTasks } from "@/lib/tasks"
import { cn } from "@/lib/utils"

const domainsKey = (id: string) => ["backupDomains", id] as const

function PasswordDialog({
  backup,
  onUnlocked,
  onCancel,
}: {
  backup: BackupSummary
  onUnlocked: (password: string) => void
  onCancel: () => void
}) {
  const queryClient = useQueryClient()
  const [password, setPassword] = useState("")
  const unlock = useMutation({
    mutationFn: () => openBackup(backup.id, password),
    onSuccess: (domains) => {
      queryClient.setQueryData(domainsKey(backup.id), domains)
      onUnlocked(password)
    },
  })
  const wrong = unlock.isError && isOrchardError(unlock.error, "backupPassword")

  return (
    <Dialog open onOpenChange={(open) => !open && onCancel()}>
      <DialogContent className="sm:max-w-sm">
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (password) unlock.mutate()
          }}
        >
          <DialogHeader>
            <div className="mb-1 flex size-9 items-center justify-center rounded-lg border bg-muted/50">
              <KeyRound className="size-4" aria-hidden="true" />
            </div>
            <DialogTitle>Encrypted backup</DialogTitle>
            <DialogDescription>
              Enter the password set for backups of {backup.deviceName} to browse its files.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-2">
            <Label htmlFor="backup-password">Backup password</Label>
            <Input
              id="backup-password"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              aria-invalid={unlock.isError || undefined}
              aria-describedby={unlock.isError ? "backup-password-error" : undefined}
              autoFocus
            />
            {unlock.isError ? (
              <p id="backup-password-error" className="text-sm text-destructive" role="alert">
                {wrong ? "That password is not correct." : errorMessage(unlock.error)}
              </p>
            ) : null}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={onCancel}>
              Cancel
            </Button>
            <Button type="submit" disabled={!password || unlock.isPending}>
              {unlock.isPending ? "Unlocking..." : "Unlock"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function DomainGroup({
  title,
  icon: Icon,
  domains,
  selected,
  onSelect,
}: {
  title: string
  icon: typeof Server
  domains: BackupDomain[]
  selected: string | null
  onSelect: (domain: string) => void
}) {
  const [open, setOpen] = useState(true)
  if (domains.length === 0) return null
  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
        >
          <ChevronRight
            className={cn("size-3.5 transition-transform duration-150", open && "rotate-90")}
            aria-hidden="true"
          />
          <Icon className="size-3.5" aria-hidden="true" />
          {title}
          <span className="ml-auto tabular-nums">{domains.length}</span>
        </button>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul className="grid gap-0.5 pb-2 pl-3">
          {domains.map((d) => {
            const active = d.domain === selected
            return (
              <li key={d.domain}>
                <button
                  type="button"
                  aria-current={active ? "true" : undefined}
                  onClick={() => onSelect(d.domain)}
                  title={d.domain}
                  className={cn(
                    "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors duration-150 hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50",
                    active && "bg-muted font-medium",
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{d.label}</span>
                  <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
                    {formatNumber(d.fileCount)}
                  </span>
                </button>
              </li>
            )
          })}
        </ul>
      </CollapsibleContent>
    </Collapsible>
  )
}

function FilePane({
  backup,
  domain,
  onExtract,
}: {
  backup: BackupSummary
  domain: BackupDomain
  onExtract: (fileIds: string[]) => void
}) {
  const [search, setSearch] = useState("")
  const query = useDeferredValue(search.trim())
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const files = useQuery({
    queryKey: ["backupFiles", backup.id, domain.domain, query],
    queryFn: () => listBackupFiles(backup.id, domain.domain, query || null),
    placeholderData: (prev) => prev,
  })

  const list = files.data ?? []
  const allChecked = list.length > 0 && list.every((f) => selected.has(f.fileId))
  const someChecked = list.some((f) => selected.has(f.fileId))
  const toggle = (id: string, on: boolean) =>
    setSelected((s) => {
      const next = new Set(s)
      if (on) next.add(id)
      else next.delete(id)
      return next
    })

  return (
    <section className="flex min-w-0 flex-1 flex-col" aria-labelledby="domain-title">
      <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2.5">
        <div className="mr-auto grid min-w-0">
          <h2 id="domain-title" className="truncate text-sm font-medium">
            {domain.label}
          </h2>
          <p className="truncate text-xs text-muted-foreground">
            {domain.domain} · {formatNumber(domain.fileCount)} files
            {domain.size ? ` · ${formatBytes(domain.size)}` : null}
          </p>
        </div>
        <InputGroup className="h-7 w-56">
          <InputGroupAddon>
            <Search aria-hidden="true" />
          </InputGroupAddon>
          <InputGroupInput
            placeholder="Search paths"
            aria-label="Search paths"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </InputGroup>
        <Button size="sm" disabled={selected.size === 0} onClick={() => onExtract([...selected])}>
          <Download aria-hidden="true" />
          Extract{selected.size ? ` ${selected.size}` : ""}
        </Button>
      </div>
      {files.isPending ? (
        <div className="grid gap-2 p-4" aria-busy="true" aria-label="Loading files">
          {Array.from({ length: 8 }, (_, i) => (
            <Skeleton key={i} className="h-8" />
          ))}
        </div>
      ) : files.isError ? (
        <p className="p-4 text-sm text-destructive" role="alert">
          {errorMessage(files.error)}
        </p>
      ) : list.length === 0 ? (
        <div className="flex flex-1 p-4">
          <EmptyState
            icon={FileSearch}
            title={query ? "No matching files" : "This domain is empty"}
            description={query ? "Try a shorter part of the path." : undefined}
          />
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          <Table>
            <TableHeader className="sticky top-0 z-10 bg-background">
              <TableRow>
                <TableHead className="w-10 pl-4">
                  <Checkbox
                    aria-label="Select all files"
                    checked={allChecked ? true : someChecked ? "indeterminate" : false}
                    onCheckedChange={(v) =>
                      setSelected(v === true ? new Set(list.map((f) => f.fileId)) : new Set())
                    }
                  />
                </TableHead>
                <TableHead>Path</TableHead>
                <TableHead className="w-24 text-right">Size</TableHead>
                <TableHead className="w-32 pr-4">Modified</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {list.map((f) => {
                const checked = selected.has(f.fileId)
                return (
                  <TableRow key={f.fileId} data-state={checked ? "selected" : undefined}>
                    <TableCell className="pl-4">
                      <Checkbox
                        aria-label={`Select ${f.relativePath}`}
                        checked={checked}
                        onCheckedChange={(v) => toggle(f.fileId, v === true)}
                      />
                    </TableCell>
                    <TableCell className="max-w-0 truncate font-mono text-xs" title={f.relativePath}>
                      {f.relativePath}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">{formatBytes(f.size)}</TableCell>
                    <TableCell className="pr-4 text-muted-foreground" title={formatDateTime(f.modified)}>
                      {formatDate(f.modified)}
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        </div>
      )}
    </section>
  )
}

/** Browses one backup: unlock, domain tree, file table, extract and restore. */
export function BackupBrowser({
  backup,
  device,
  onBack,
}: {
  backup: BackupSummary
  device: Device | null
  onBack: () => void
}) {
  const tasks = useTasks()
  const queryClient = useQueryClient()
  const [password, setPassword] = useState<string | null>(null)
  const locked = backup.encrypted && password === null
  const domains = useQuery({
    queryKey: domainsKey(backup.id),
    queryFn: () => openBackup(backup.id, password),
    enabled: !locked,
    staleTime: Infinity,
  })
  const [selected, setSelected] = useState<string | null>(null)
  const [restoreOpen, setRestoreOpen] = useState(false)

  const list = domains.data ?? []
  const current = list.find((d) => d.domain === selected) ?? list[0] ?? null
  const system = list.filter((d) => d.kind === "system")
  const apps = list.filter((d) => d.kind === "app")

  const extract = async (fileIds: string[]) => {
    const settings = await queryClient.fetchQuery({ queryKey: ["settings"], queryFn: getSettings })
    const dest = settings.downloadDir ?? (await pickFolder("Extract files to"))
    if (!dest) return
    void tasks.start({
      kind: "export",
      title: `Extract ${fileIds.length} ${fileIds.length === 1 ? "file" : "files"}`,
      successMessage: `Extracted to ${dest}`,
      output: dest,
      run: (taskId) => extractBackupFiles(taskId, backup.id, fileIds, dest),
    })
  }

  const restore = () => {
    if (!device) return
    const udid = device.udid
    void tasks.start({
      kind: "restore",
      title: `Restore ${device.name}`,
      successMessage: `${device.name} restored`,
      run: (taskId) => restoreBackup(taskId, udid, backup.id, password),
    })
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-3 border-b px-4 py-2.5">
        <Button variant="ghost" size="sm" onClick={onBack}>
          <ArrowLeft aria-hidden="true" />
          Backups
        </Button>
        <div className="mr-auto grid min-w-0">
          <p className="flex items-center gap-2 truncate text-sm font-medium">
            {backup.deviceName}
            {backup.encrypted ? (
              <Badge variant="outline" className="gap-1 font-normal text-muted-foreground">
                <Lock aria-hidden="true" /> Encrypted
              </Badge>
            ) : null}
          </p>
          <p className="truncate text-xs text-muted-foreground">
            iOS {backup.iosVersion} · {formatDateTime(backup.date)} · {formatBytes(backup.size)}
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          disabled={!device || locked || domains.isPending}
          onClick={() => setRestoreOpen(true)}
          title={device ? undefined : "Connect a device to restore"}
        >
          <RotateCcw aria-hidden="true" />
          {device ? `Restore to ${device.name}` : "Restore to device"}
        </Button>
      </div>

      {locked ? (
        <>
          <div className="flex flex-1 p-4 md:p-6">
            <EmptyState icon={Lock} title="Backup is locked" description="Enter the backup password to browse it." />
          </div>
          <PasswordDialog backup={backup} onUnlocked={setPassword} onCancel={onBack} />
        </>
      ) : domains.isPending ? (
        <div className="flex min-h-0 flex-1" aria-busy="true" aria-label="Reading backup">
          <div className="grid w-64 content-start gap-2 border-r p-3">
            {Array.from({ length: 10 }, (_, i) => (
              <Skeleton key={i} className="h-7" />
            ))}
          </div>
          <div className="grid flex-1 content-start gap-2 p-4">
            {Array.from({ length: 8 }, (_, i) => (
              <Skeleton key={i} className="h-8" />
            ))}
          </div>
        </div>
      ) : domains.isError ? (
        <div className="flex flex-1 p-4 md:p-6">
          <EmptyState icon={FolderTree} title="Could not open this backup" description={errorMessage(domains.error)} />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 animate-in fade-in-0 duration-200">
          <nav aria-label="Backup domains" className="w-64 shrink-0 overflow-y-auto border-r p-2">
            <DomainGroup
              title="System"
              icon={Server}
              domains={system}
              selected={current?.domain ?? null}
              onSelect={setSelected}
            />
            <DomainGroup
              title="Apps"
              icon={LayoutGrid}
              domains={apps}
              selected={current?.domain ?? null}
              onSelect={setSelected}
            />
          </nav>
          {current ? (
            <FilePane key={current.domain} backup={backup} domain={current} onExtract={(ids) => void extract(ids)} />
          ) : (
            <div className="flex flex-1 p-4">
              <EmptyState icon={FolderTree} title="No domains" description="This backup has no files." />
            </div>
          )}
        </div>
      )}

      {device ? (
        <ConfirmDialog
          open={restoreOpen}
          onOpenChange={setRestoreOpen}
          title={`Restore ${device.name} from this backup?`}
          description={
            <>
              Everything on {device.name} is replaced with the contents of the backup from{" "}
              {formatDateTime(backup.date)}. Data created since then is lost. The device restarts when the restore
              finishes.
            </>
          }
          actionLabel="Erase and restore"
          confirmText={device.name}
          onConfirm={restore}
        />
      ) : null}
    </div>
  )
}
