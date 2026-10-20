import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { Check, Copy, ExternalLink, FolderOpen, Laptop, LogOut, Moon, Server, Sun } from "lucide-react"
import { useState } from "react"
import { toast } from "sonner"

import { PageBody, PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import {
  appInfo,
  currentBackend,
  errorMessage,
  getSettings,
  openDataDir,
  pickFolder,
  setDownloadDir,
  type Settings,
} from "@/lib/api"
import { disconnect, serverUrl } from "@/lib/remote"
import { useTheme, type Theme } from "@/lib/theme"
import { cn } from "@/lib/utils"

const THEMES: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Laptop },
]

function ThemePicker() {
  const { theme, setTheme } = useTheme()
  return (
    <div role="radiogroup" aria-label="Theme" className="grid grid-cols-3 gap-2 sm:max-w-md">
      {THEMES.map(({ value, label, icon: Icon }) => {
        const checked = theme === value
        return (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={checked ? 0 : -1}
            onClick={() => setTheme(value)}
            onKeyDown={(e) => {
              const i = THEMES.findIndex((t) => t.value === value)
              let next = -1
              if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (i + 1) % THEMES.length
              if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = (i + THEMES.length - 1) % THEMES.length
              if (next >= 0) {
                e.preventDefault()
                setTheme(THEMES[next].value)
                const sibling = e.currentTarget.parentElement?.children[next] as HTMLElement | undefined
                sibling?.focus()
              }
            }}
            className={cn(
              "flex flex-col items-center gap-2 rounded-lg border p-3 text-sm transition-colors duration-150 outline-none hover:bg-muted/60 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
              checked && "border-primary bg-muted/60 font-medium",
            )}
          >
            <Icon className="size-4" aria-hidden="true" />
            {label}
          </button>
        )
      })}
    </div>
  )
}

function PathField({ value, placeholder }: { value: string | null; placeholder: string }) {
  return (
    <p
      className={cn(
        "h-7 min-w-0 flex-1 truncate rounded-lg border bg-muted/40 px-2.5 font-mono text-xs leading-[1.625rem]",
        !value && "font-sans text-muted-foreground",
      )}
      title={value ?? undefined}
    >
      {value ?? placeholder}
    </p>
  )
}

function DownloadsCard({ settings }: { settings: Settings }) {
  const queryClient = useQueryClient()
  const update = useMutation({
    mutationFn: (path: string | null) => setDownloadDir(path),
    onSuccess: (next) => {
      queryClient.setQueryData(["settings"], next)
      toast.success(next.downloadDir ? "Download folder updated" : "Orchard will ask where to save")
    },
    onError: (e) => toast.error("Could not update the download folder", { description: errorMessage(e) }),
  })

  const change = async () => {
    const path = await pickFolder("Default download folder", settings.downloadDir)
    if (path) update.mutate(path)
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm">Downloads</CardTitle>
        <CardDescription>Where IPAs, exports and extracted files are saved by default.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 sm:flex-row sm:items-center">
        <PathField
          value={settings.downloadDir}
          placeholder={currentBackend() === "remote" ? "Download each result to this device" : "Ask every time"}
        />
        <div className="flex shrink-0 gap-2">
          {/* A remote client cannot browse the server's folders. */}
          {currentBackend() === "remote" ? null : (
            <Button variant="outline" size="sm" onClick={() => void change()} disabled={update.isPending}>
              <FolderOpen aria-hidden="true" />
              Change...
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => update.mutate(null)}
            disabled={update.isPending || settings.downloadDir === null}
          >
            Reset
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}

function ServerCard() {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm">Server</CardTitle>
        <CardDescription>This client works through an Orchard server; devices connect to it.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 sm:flex-row sm:items-center">
        <div className="flex min-w-0 flex-1 items-center gap-2 rounded-md border bg-muted/40 px-3 py-1.5">
          <Server className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span className="truncate font-mono text-sm">{serverUrl()}</span>
        </div>
        <Button variant="outline" size="sm" className="shrink-0" onClick={disconnect}>
          <LogOut aria-hidden="true" />
          Disconnect
        </Button>
      </CardContent>
    </Card>
  )
}

function StorageCard({ settings }: { settings: Settings }) {
  const [copied, setCopied] = useState(false)
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm">Storage</CardTitle>
        <CardDescription>Pairing records, App Store session, backups and cache live here.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 sm:flex-row sm:items-center">
        <PathField value={settings.dataDir} placeholder="" />
        <div className="flex shrink-0 gap-2">
          <Button
            variant="outline"
            size="sm"
            aria-label="Copy data folder path"
            onClick={() => {
              void navigator.clipboard.writeText(settings.dataDir).then(() => {
                setCopied(true)
                toast.success("Path copied")
                setTimeout(() => setCopied(false), 1500)
              })
            }}
          >
            {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
            Copy
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={currentBackend() !== "desktop"}
            onClick={() =>
              void openDataDir().catch((e) =>
                toast.error("Could not open folder", { description: errorMessage(e) }),
              )
            }
          >
            <ExternalLink aria-hidden="true" />
            Open
          </Button>
        </div>
      </CardContent>
      <CardContent>
        <p className="text-xs text-muted-foreground">
          App Store sign-in:{" "}
          {settings.storeBackend === "ipatool" ? (
            <span className="text-foreground">via ipatool</span>
          ) : (
            <>
              <span className="text-foreground">built-in client</span>. Install{" "}
              <span className="font-mono text-foreground">ipatool</span> for reliable sign-in.
            </>
          )}
        </p>
      </CardContent>
    </Card>
  )
}

function AboutCard() {
  const { data: info } = useQuery({ queryKey: ["appInfo"], queryFn: appInfo, staleTime: Infinity })
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm">About</CardTitle>
        <CardDescription>Manage iPhone and iPad from any desktop: apps, files, backups and App Store downloads.</CardDescription>
      </CardHeader>
      <CardContent>
        <dl className="divide-y border-t text-sm">
          <div className="grid grid-cols-[8rem_1fr] gap-3 py-2.5">
            <dt className="text-muted-foreground">Application</dt>
            <dd className="font-medium">Orchard</dd>
          </div>
          <div className="grid grid-cols-[8rem_1fr] gap-3 py-2.5">
            <dt className="text-muted-foreground">Version</dt>
            <dd className="tabular-nums">{info ? info.version : <Skeleton className="h-4 w-12" />}</dd>
          </div>
          <div className="grid grid-cols-[8rem_1fr] gap-3 py-2.5">
            <dt className="text-muted-foreground">Platform</dt>
            <dd>{info ? `${info.platform} (${info.arch})` : <Skeleton className="h-4 w-32" />}</dd>
          </div>
          <div className="grid grid-cols-[8rem_1fr] gap-3 py-2.5">
            <dt className="text-muted-foreground">Library sync</dt>
            <dd className="text-muted-foreground">
              Music, TV and Books library sync is not supported: it uses Apple's private sync protocol. Use Media to
              send files to apps like VLC instead.
            </dd>
          </div>
          <div className="grid grid-cols-[8rem_1fr] gap-3 py-2.5">
            <dt className="text-muted-foreground">License</dt>
            <dd>MIT</dd>
          </div>
          <div className="grid grid-cols-[8rem_1fr] gap-3 py-2.5">
            <dt className="text-muted-foreground">Source</dt>
            <dd className="min-w-0 truncate">
              <a
                href="https://github.com/berry-13/orchard"
                target="_blank"
                rel="noreferrer"
                className="rounded-sm underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
              >
                github.com/berry-13/orchard
              </a>
            </dd>
          </div>
        </dl>
      </CardContent>
    </Card>
  )
}

export default function SettingsScreen() {
  const { data: settings, isPending, isError, error } = useQuery({ queryKey: ["settings"], queryFn: getSettings })

  return (
    <>
      <PageHeader title="Settings" />
      <PageBody className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-4 p-4 md:p-6">
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Appearance</CardTitle>
            <CardDescription>System follows your operating system setting.</CardDescription>
          </CardHeader>
          <CardContent>
            <ThemePicker />
          </CardContent>
        </Card>
        {isPending ? (
          <>
            <Skeleton className="h-36" />
            <Skeleton className="h-36" />
          </>
        ) : isError ? (
          <p className="text-sm text-destructive" role="alert">
            {errorMessage(error)}
          </p>
        ) : (
          <>
            {currentBackend() === "remote" ? <ServerCard /> : null}
            <DownloadsCard settings={settings} />
            <StorageCard settings={settings} />
          </>
        )}
        <AboutCard />
      </PageBody>
    </>
  )
}
