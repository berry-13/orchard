import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { Check, Copy, ExternalLink, FileKey, FolderOpen, Laptop, LogOut, Moon, Sun } from "lucide-react"
import { useId, useState } from "react"
import { toast } from "sonner"

import { PageBody, PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import { canPairOverWifi, WifiPairButton } from "@/components/wifi-pair-dialog"
import {
  appInfo,
  currentBackend,
  errorMessage,
  getSettings,
  importPairingFile,
  openDataDir,
  pickFiles,
  pickFolder,
  setDownloadDir,
  type Settings,
} from "@/lib/api"
import { disconnect, serverUrl } from "@/lib/remote"
import { useTheme, type Theme } from "@/lib/theme"
import { cn } from "@/lib/utils"

/** A titled group of rows, like a section of the system Settings app. */
function Section({
  title,
  footer,
  children,
}: {
  title: string
  footer?: React.ReactNode
  children: React.ReactNode
}) {
  const id = useId()
  return (
    <section aria-labelledby={id} className="grid gap-2">
      <h2 id={id} className="px-1 text-xs font-medium tracking-wide text-muted-foreground uppercase">
        {title}
      </h2>
      <div className="divide-y rounded-xl border bg-card text-card-foreground">{children}</div>
      {footer ? <p className="px-1 text-xs leading-relaxed text-muted-foreground">{footer}</p> : null}
    </section>
  )
}

/** One setting: what it is on the left, its value or controls on the right. */
function Row({
  title,
  description,
  children,
  stacked = false,
}: {
  title: string
  description?: React.ReactNode
  children?: React.ReactNode
  /** Puts the controls under the text, for wide values such as paths. */
  stacked?: boolean
}) {
  return (
    <div
      className={cn(
        "flex gap-3 px-4 py-3",
        stacked ? "flex-col" : "flex-col sm:flex-row sm:items-center sm:justify-between",
      )}
    >
      <div className="grid min-w-0 gap-0.5">
        <p className="text-sm font-medium">{title}</p>
        {description ? <p className="text-xs leading-relaxed text-muted-foreground">{description}</p> : null}
      </div>
      {children ? <div className="flex min-w-0 shrink-0 flex-wrap items-center gap-2">{children}</div> : null}
    </div>
  )
}

const THEMES: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Laptop },
]

/** A segmented control for the theme, with arrow keys moving the choice. */
function ThemePicker() {
  const { theme, setTheme } = useTheme()
  return (
    <div role="radiogroup" aria-label="Theme" className="inline-flex rounded-lg bg-muted p-0.5">
      {THEMES.map(({ value, label, icon: Icon }, i) => {
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
              "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm text-muted-foreground transition-colors duration-150 outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50",
              checked && "bg-background font-medium text-foreground shadow-sm",
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
        "min-w-0 flex-1 truncate rounded-lg border bg-muted/40 px-2.5 py-1.5 font-mono text-xs",
        !value && "font-sans text-muted-foreground",
      )}
      title={value ?? undefined}
    >
      {value ?? placeholder}
    </p>
  )
}

function DownloadRow({ settings }: { settings: Settings }) {
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
    <Row title="Download folder" description="Where IPAs, exports and extracted files are saved." stacked>
      <PathField
        value={settings.downloadDir}
        placeholder={currentBackend() === "remote" ? "Download each result to this device" : "Ask every time"}
      />
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
    </Row>
  )
}

function DataRow({ settings }: { settings: Settings }) {
  const [copied, setCopied] = useState(false)
  return (
    <Row
      title="Data folder"
      description="Pairing records, the App Store session, backups and the cache."
      stacked
    >
      <PathField value={settings.dataDir} placeholder="" />
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
      {currentBackend() === "desktop" ? (
        <Button
          variant="outline"
          size="sm"
          onClick={() =>
            void openDataDir().catch((e) => toast.error("Could not open folder", { description: errorMessage(e) }))
          }
        >
          <ExternalLink aria-hidden="true" />
          Open
        </Button>
      ) : null}
    </Row>
  )
}

function ImportPairingButton() {
  const queryClient = useQueryClient()
  const importFile = useMutation({
    mutationFn: importPairingFile,
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: ["devices"] })
      toast.success(result.kind === "wifi" ? "Wi-Fi pairing imported" : "Pairing record imported", {
        description: result.id,
      })
    },
    onError: (e) => toast.error("Could not import the pairing file", { description: errorMessage(e) }),
  })

  const choose = async () => {
    const [path] = await pickFiles("Choose a pairing file")
    if (path) importFile.mutate(path)
  }

  return (
    <Button variant="outline" size="sm" onClick={() => void choose()} disabled={importFile.isPending}>
      <FileKey aria-hidden="true" />
      {importFile.isPending ? "Importing..." : "Import..."}
    </Button>
  )
}

function AboutSection() {
  const { data: info } = useQuery({ queryKey: ["appInfo"], queryFn: appInfo, staleTime: Infinity })
  return (
    <Section
      title="About"
      footer="Music, TV and Books library sync is not supported: it uses Apple's private sync protocol. Use Media to send files to apps like VLC instead."
    >
      <Row title="Version">
        <span className="text-sm text-muted-foreground tabular-nums">
          {info ? info.version : <Skeleton className="h-4 w-12" />}
        </span>
      </Row>
      <Row title="Platform">
        <span className="text-sm text-muted-foreground">
          {info ? `${info.platform} (${info.arch})` : <Skeleton className="h-4 w-32" />}
        </span>
      </Row>
      <Row title="License">
        <span className="text-sm text-muted-foreground">MIT</span>
      </Row>
      <Row title="Source code">
        <a
          href="https://github.com/berry-13/orchard"
          target="_blank"
          rel="noreferrer"
          className="flex items-center gap-1 rounded-sm text-sm text-primary underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
        >
          github.com/berry-13/orchard
          <ExternalLink className="size-3.5" aria-hidden="true" />
        </a>
      </Row>
    </Section>
  )
}

export default function SettingsScreen() {
  const { data: settings, isPending, isError, error } = useQuery({ queryKey: ["settings"], queryFn: getSettings })
  const remote = currentBackend() === "remote"

  return (
    <>
      <PageHeader title="Settings" />
      <PageBody className="mx-auto flex w-full max-w-2xl flex-1 flex-col gap-8 p-4 md:p-6">
        <Section title="Appearance">
          <Row title="Theme" description="System follows the device's setting.">
            <ThemePicker />
          </Row>
        </Section>

        {remote ? (
          <Section title="Server" footer="Devices connect to the server; this app controls it from here.">
            <Row title="Connected to" description={<span className="font-mono">{serverUrl()}</span>}>
              <Button variant="outline" size="sm" onClick={disconnect}>
                <LogOut aria-hidden="true" />
                Disconnect
              </Button>
            </Row>
          </Section>
        ) : null}

        <Section title="Devices">
          {canPairOverWifi() ? (
            <Row
              title="Pair over Wi-Fi"
              description="For iOS 27 devices on this iPad's network or hotspot, with Developer Mode on."
            >
              <WifiPairButton label="Pair..." />
            </Row>
          ) : null}
          <Row
            title="Import a pairing"
            description="A record from another computer, or an iOS 27 Wi-Fi pairing exported by an app such as StikPair."
          >
            <ImportPairingButton />
          </Row>
        </Section>

        {isPending ? (
          <Skeleton className="h-44 rounded-xl" aria-label="Loading settings" />
        ) : isError ? (
          <p className="text-sm text-destructive" role="alert">
            {errorMessage(error)}
          </p>
        ) : (
          <>
            <Section title="App Store">
              <Row
                title="Sign-in method"
                description={
                  settings.storeBackend === "ipatool" ? (
                    "Through ipatool."
                  ) : (
                    <>
                      Orchard's built-in client. Install <span className="font-mono">ipatool</span> for more reliable
                      sign-in.
                    </>
                  )
                }
              >
                <span className="text-sm text-muted-foreground">
                  {settings.storeBackend === "ipatool" ? "ipatool" : "Built-in"}
                </span>
              </Row>
            </Section>

            <Section title="Files">
              <DownloadRow settings={settings} />
              <DataRow settings={settings} />
            </Section>
          </>
        )}

        <AboutSection />
      </PageBody>
    </>
  )
}
