import { useQuery, useQueryClient } from "@tanstack/react-query"
import { ChevronDown, FileWarning, Lock, LockOpen, PackagePlus, PenLine, Upload } from "lucide-react"
import { useMemo, useState } from "react"
import { toast } from "sonner"

import {
  EMPTY_RESIGN,
  ResignFields,
  ResignIssues,
  type ResignForm,
} from "@/components/resign-section"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { Skeleton } from "@/components/ui/skeleton"
import { activeDropLabel, useDragging, useDropTarget } from "@/hooks/use-window-drop"
import {
  checkResign,
  errorMessage,
  inspectIpa,
  installIpa,
  pickIpa,
  pickSaveIpa,
  resignAndInstall,
  resignIpa,
  revealPath,
  type IpaInfo,
  type ResignChanges,
} from "@/lib/api"
import { useDevices } from "@/lib/devices"
import { formatBytes } from "@/lib/format"
import { useTasks } from "@/lib/tasks"
import { useUi, type IpaMode } from "@/lib/ui"
import { cn } from "@/lib/utils"

/** Opens the IPA picker and shows the sheet for the chosen file. */
export function useInstallIpa(mode: IpaMode = "install") {
  const { setIpaPath } = useUi()
  return async () => {
    const path = await pickIpa()
    if (path) setIpaPath(path, mode)
  }
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[7rem_1fr] items-baseline gap-2 py-2 text-sm">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate font-medium">{children}</dd>
    </div>
  )
}

function changes(form: ResignForm): ResignChanges {
  return {
    bundleId: form.bundleId.trim() || null,
    displayName: form.displayName.trim() || null,
  }
}

function IpaSummary({ ipa, installOn }: { ipa: IpaInfo; installOn: string | null }) {
  return (
    <>
      <div className="flex items-center gap-3">
        {ipa.icon ? (
          <img src={ipa.icon} alt="" className="size-16 rounded-[22%] border" />
        ) : (
          <div className="flex size-16 items-center justify-center rounded-[22%] border bg-muted">
            <PackagePlus className="size-6 text-muted-foreground" aria-hidden="true" />
          </div>
        )}
        <div className="min-w-0">
          <p className="truncate text-base font-semibold">{ipa.name}</p>
          <p className="truncate font-mono text-xs text-muted-foreground">{ipa.bundleId}</p>
          <div className="mt-1.5">
            {ipa.encrypted ? (
              <Badge variant="secondary" className="gap-1">
                <Lock aria-hidden="true" /> Encrypted
              </Badge>
            ) : (
              <Badge variant="outline" className="gap-1 text-muted-foreground">
                <LockOpen aria-hidden="true" /> Decrypted
              </Badge>
            )}
          </div>
        </div>
      </div>
      <dl className="divide-y rounded-lg border px-3">
        <Field label="Version">
          {ipa.shortVersion} <span className="text-muted-foreground">({ipa.version})</span>
        </Field>
        <Field label="Minimum iOS">{ipa.minimumOsVersion ?? "Not specified"}</Field>
        <Field label="Size">{formatBytes(ipa.size)}</Field>
        {installOn !== null ? <Field label="Install on">{installOn}</Field> : null}
      </dl>
    </>
  )
}

/**
 * The IPA sheet: inspection, optional re-signing, then install on the
 * selected device ("install" mode) or save a re-signed copy ("resign" mode).
 */
export function IpaInstallSheet() {
  const { ipaPath, ipaMode, setIpaPath } = useUi()
  return (
    <Sheet open={!!ipaPath} onOpenChange={(open) => !open && setIpaPath(null)}>
      <SheetContent className="w-full gap-0 sm:max-w-md">
        {ipaPath ? <IpaSheetBody key={`${ipaMode}:${ipaPath}`} path={ipaPath} mode={ipaMode} /> : null}
      </SheetContent>
    </Sheet>
  )
}

function IpaSheetBody({ path, mode }: { path: string; mode: IpaMode }) {
  const { setIpaPath } = useUi()
  const { device } = useDevices()
  const tasks = useTasks()
  const queryClient = useQueryClient()
  const [resignOpen, setResignOpen] = useState(mode === "resign")
  const [form, setForm] = useState<ResignForm>(EMPTY_RESIGN)
  const info = useQuery({
    queryKey: ["inspectIpa", path],
    queryFn: () => inspectIpa(path),
    retry: false,
  })

  const udid = mode === "install" ? (device?.udid ?? null) : null
  const ready = !!form.identity && !!form.profile
  const bundleOverride = form.bundleId.trim() || null
  const check = useQuery({
    queryKey: ["checkResign", path, form.identity?.id, form.profile?.id, udid, bundleOverride],
    queryFn: () => checkResign(path, form.identity!.id, form.profile!.id, udid, bundleOverride),
    enabled: resignOpen && ready && !!info.data,
    retry: false,
  })

  const ipa = info.data
  const encrypted = !!ipa?.encrypted
  const resigning = resignOpen
  const blocked = resigning && encrypted
  const fileName = path.split(/[\\/]/).pop() ?? ""

  const install = () => {
    if (!device || !ipa) return
    const target = device
    setIpaPath(null)
    void tasks
      .start({
        kind: "install",
        title: resigning ? `Re-sign and install ${ipa.name}` : `Install ${ipa.name}`,
        successMessage: `${form.displayName.trim() || ipa.name} installed on ${target.name}`,
        run: (taskId) =>
          resigning
            ? resignAndInstall(taskId, target.udid, ipa.path, form.identity!.id, form.profile!.id, changes(form))
            : installIpa(taskId, target.udid, ipa.path),
      })
      .then(() => queryClient.invalidateQueries({ queryKey: ["apps", target.udid] }))
  }

  const saveResigned = async () => {
    if (!ipa || !form.identity || !form.profile) return
    const stem = fileName.replace(/\.ipa$/i, "")
    const dest = await pickSaveIpa(`${stem}-resigned.ipa`)
    if (!dest) return
    const identityId = form.identity.id
    const profileId = form.profile.id
    const edits = changes(form)
    setIpaPath(null)
    const report = await tasks.start({
      kind: "export",
      title: `Re-sign ${ipa.name}`,
      successMessage: false,
      run: (taskId) => resignIpa(taskId, ipa.path, identityId, profileId, edits, dest),
    })
    if (report) {
      toast.success(`Re-signed ${ipa.name}`, {
        description: `${report.bundleId}, ${formatBytes(report.outputSize)}`,
        action: { label: "Show", onClick: () => void revealPath(report.path) },
      })
    }
  }

  const canInstall = !!ipa && !!device && !blocked && (!resigning || ready)
  const canSave = !!ipa && !encrypted && ready

  const resignBody = ipa ? (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-3">
      {encrypted ? (
        <ResignIssues issues={[{ kind: "encrypted" }]} deviceName={null} />
      ) : (
        <>
          <ResignFields
            value={form}
            onChange={setForm}
            currentBundleId={ipa.bundleId}
            currentName={ipa.name}
            udid={udid}
            deviceName={mode === "install" ? (device?.name ?? null) : null}
          />
          {check.isFetching && !check.data ? (
            <Skeleton className="h-9 w-full" aria-label="Checking the signing setup" />
          ) : check.isError ? (
            <p className="text-xs text-destructive" role="alert">
              {errorMessage(check.error)}
            </p>
          ) : check.data ? (
            check.data.length ? (
              <ResignIssues issues={check.data} deviceName={device?.name ?? null} />
            ) : (
              <p className="text-xs text-muted-foreground" role="status">
                No problems found with this certificate and profile.
              </p>
            )
          ) : null}
        </>
      )}
    </div>
  ) : null

  return (
    <>
      <SheetHeader className="border-b">
        <SheetTitle>{mode === "resign" ? "Re-sign IPA" : "Install IPA"}</SheetTitle>
        <SheetDescription className="truncate" title={path}>
          {fileName}
        </SheetDescription>
      </SheetHeader>
      <div className="flex-1 overflow-x-hidden overflow-y-auto p-4">
        {info.isPending ? (
          <div className="grid gap-4" aria-busy="true" aria-label="Reading IPA">
            <div className="flex items-center gap-3">
              <Skeleton className="size-16 rounded-[22%]" />
              <div className="grid flex-1 gap-2">
                <Skeleton className="h-4 w-1/2" />
                <Skeleton className="h-3 w-2/3" />
              </div>
            </div>
            <Skeleton className="h-32 w-full" />
          </div>
        ) : info.isError ? (
          <div className="flex flex-col items-center gap-3 rounded-lg border border-dashed p-8 text-center" role="alert">
            <FileWarning className="size-6 text-destructive" aria-hidden="true" />
            <p className="text-sm font-medium">Could not read this IPA</p>
            <p className="text-sm text-muted-foreground">{errorMessage(info.error)}</p>
          </div>
        ) : (
          <div className="grid grid-cols-[minmax(0,1fr)] gap-4 animate-in fade-in-0 duration-200">
            <IpaSummary ipa={info.data} installOn={mode === "install" ? (device?.name ?? "No device connected") : null} />
            {mode === "resign" ? (
              resignBody
            ) : (
              <Collapsible open={resignOpen} onOpenChange={setResignOpen} className="rounded-lg border">
                <CollapsibleTrigger asChild>
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 rounded-lg px-3 py-2.5 text-left text-sm font-medium outline-none hover:bg-muted/50 focus-visible:ring-3 focus-visible:ring-ring/50"
                  >
                    <PenLine className="size-4 text-muted-foreground" aria-hidden="true" />
                    <span className="flex-1">Re-sign before installing</span>
                    <ChevronDown
                      className={cn(
                        "size-4 text-muted-foreground transition-transform duration-200",
                        resignOpen && "rotate-180",
                      )}
                      aria-hidden="true"
                    />
                  </button>
                </CollapsibleTrigger>
                <CollapsibleContent className="border-t p-3">{resignBody}</CollapsibleContent>
              </Collapsible>
            )}
            {mode === "install" && !resignOpen && encrypted ? (
              <p className="text-xs text-muted-foreground">
                Encrypted IPAs install only on devices signed in with the Apple ID that bought them.
              </p>
            ) : null}
          </div>
        )}
      </div>
      <SheetFooter className="border-t sm:flex-row sm:justify-end">
        <Button variant="outline" onClick={() => setIpaPath(null)}>
          Cancel
        </Button>
        {mode === "resign" ? (
          <Button onClick={() => void saveResigned()} disabled={!canSave}>
            <PenLine aria-hidden="true" />
            Re-sign and save...
          </Button>
        ) : (
          <Button onClick={install} disabled={!canInstall}>
            <Upload aria-hidden="true" />
            {resigning ? "Re-sign and install" : "Install"}
          </Button>
        )}
      </SheetFooter>
    </>
  )
}

/** Window wide IPA drop target and the overlay shown while dragging. */
export function DropOverlay() {
  const { setIpaPath } = useUi()
  const dragging = useDragging()
  const target = useMemo(
    () => ({
      label: "Drop an IPA to install it",
      handler: (paths: string[]) => {
        const ipa = paths.find((p) => p.toLowerCase().endsWith(".ipa"))
        if (!ipa) return false
        setIpaPath(ipa)
        return true
      },
    }),
    [setIpaPath],
  )
  useDropTarget(target)

  if (!dragging) return null
  return (
    <div
      className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center bg-background/70 p-6 backdrop-blur-sm animate-in fade-in-0 duration-150"
      aria-hidden="true"
    >
      <div className="flex h-full w-full flex-col items-center justify-center gap-3 rounded-xl border-2 border-dashed border-foreground/20">
        <div className="flex size-12 items-center justify-center rounded-xl border bg-background">
          <Upload className="size-5" />
        </div>
        <p className="text-sm font-medium">{activeDropLabel()}</p>
      </div>
    </div>
  )
}
