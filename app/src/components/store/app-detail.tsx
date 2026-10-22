import { useQuery } from "@tanstack/react-query"
import { Download, ExternalLink, QrCode, Smartphone } from "lucide-react"
import { QRCodeSVG } from "qrcode.react"
import { useState } from "react"

import { Rating } from "@/components/store/rating"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { Skeleton } from "@/components/ui/skeleton"
import { errorMessage, storeVersions, type Device, type StoreApp } from "@/lib/api"
import { formatBytes, formatNumber } from "@/lib/format"

const LATEST = "latest"

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[7rem_1fr] items-baseline gap-2 py-2 text-sm">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate font-medium">{children}</dd>
    </div>
  )
}

/** A QR code of the app's App Store link, for a phone's camera to open it there. */
function StoreQrCode({ app }: { app: StoreApp }) {
  const url = `https://apps.apple.com/app/id${app.appId}`
  return (
    <Dialog>
      <DialogTrigger asChild>
        <Button variant="outline">
          <QrCode aria-hidden="true" />
          Show QR code
        </Button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Get {app.name} on an iPhone</DialogTitle>
          <DialogDescription>
            Scan this with the iPhone's camera to open the app in its App Store, then tap Get.
          </DialogDescription>
        </DialogHeader>
        <div className="mx-auto rounded-lg bg-white p-4">
          <QRCodeSVG
            value={url}
            size={240}
            marginSize={0}
            bgColor="#ffffff"
            fgColor="#000000"
            role="img"
            aria-label={`QR code for ${app.name} in the App Store`}
          />
        </div>
        <p className="truncate text-center font-mono text-xs text-muted-foreground">{url}</p>
      </DialogContent>
    </Dialog>
  )
}

function DetailBody({
  app,
  device,
  onDownload,
  onInstall,
  onOpenOnDevice,
}: {
  app: StoreApp
  device: Device | null
  onDownload: (versionId: string | null) => void
  onInstall: (versionId: string | null) => void
  onOpenOnDevice: () => void
}) {
  const [version, setVersion] = useState(LATEST)
  const versions = useQuery({
    queryKey: ["storeVersions", app.appId],
    queryFn: () => storeVersions(app.appId),
    staleTime: 5 * 60_000,
  })
  const versionId = version === LATEST ? null : version

  return (
    <>
      <SheetHeader className="border-b">
        <div className="flex items-start gap-4 pr-8">
          <img src={app.artworkUrl} alt="" className="size-20 shrink-0 rounded-[22%] border" />
          <div className="grid min-w-0 gap-1">
            <SheetTitle className="text-base leading-snug">{app.name}</SheetTitle>
            <SheetDescription className="truncate">{app.seller}</SheetDescription>
            <div className="flex flex-wrap items-center gap-2 pt-1">
              <Badge variant={app.price > 0 ? "default" : "secondary"}>{app.formattedPrice}</Badge>
              <Badge variant="outline" className="font-normal text-muted-foreground">
                {app.genre}
              </Badge>
            </div>
          </div>
        </div>
      </SheetHeader>
      <div className="grid flex-1 content-start gap-5 overflow-y-auto px-4">
        <Rating value={app.averageRating} count={app.ratingCount} />
        <dl className="divide-y rounded-lg border px-3">
          <Field label="Version">{app.version}</Field>
          <Field label="Size">{formatBytes(app.size)}</Field>
          <Field label="Minimum iOS">{app.minimumOsVersion ?? "Not specified"}</Field>
          <Field label="Ratings">{formatNumber(app.ratingCount)}</Field>
          <Field label="Bundle ID">
            <span className="font-mono text-xs">{app.bundleId}</span>
          </Field>
        </dl>
        <div className="grid gap-2">
          <Label htmlFor="store-version">Version to get</Label>
          {versions.isPending ? (
            <Skeleton className="h-8 w-full" aria-label="Loading versions" />
          ) : (
            <Select value={version} onValueChange={setVersion}>
              <SelectTrigger id="store-version" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={LATEST}>Latest ({app.version})</SelectItem>
                {(versions.data ?? []).slice(1).map((v) => (
                  <SelectItem key={v.id} value={v.id}>
                    {v.version ?? "Version id"}{" "}
                    <span className="font-mono text-xs text-muted-foreground">{v.id}</span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          {versions.isError ? (
            <p className="text-xs text-destructive" role="alert">
              Could not load older versions: {errorMessage(versions.error)}
            </p>
          ) : null}
        </div>
        <section className="grid gap-1.5" aria-labelledby="store-about">
          <h3 id="store-about" className="text-sm font-medium">
            About
          </h3>
          <p className="text-sm leading-relaxed text-muted-foreground">{app.description}</p>
        </section>
      </div>
      <SheetFooter className="border-t">
        <Button onClick={() => onInstall(versionId)} disabled={!device}>
          <Smartphone aria-hidden="true" />
          <span className="truncate">{device ? `Install on ${device.name}` : "Connect a device to install"}</span>
        </Button>
        {device ? (
          <Button variant="outline" onClick={onOpenOnDevice}>
            <ExternalLink aria-hidden="true" />
            <span className="truncate">Open in App Store on {device.name}</span>
          </Button>
        ) : null}
        <StoreQrCode app={app} />
        <Button variant="outline" onClick={() => onDownload(versionId)}>
          <Download aria-hidden="true" />
          Download IPA
        </Button>
      </SheetFooter>
    </>
  )
}

/** Detail sheet for a store app with version choice, download and install. */
export function AppDetailSheet({
  app,
  open,
  device,
  onOpenChange,
  onDownload,
  onInstall,
  onOpenOnDevice,
}: {
  app: StoreApp | null
  open: boolean
  device: Device | null
  onOpenChange: (open: boolean) => void
  onDownload: (app: StoreApp, versionId: string | null) => void
  onInstall: (app: StoreApp, versionId: string | null) => void
  /** Opens the app's page in the device's App Store, to get it with the device's Apple ID. */
  onOpenOnDevice: (app: StoreApp) => void
}) {
  return (
    <Sheet open={open && app !== null} onOpenChange={onOpenChange}>
      <SheetContent className="w-full gap-4 sm:max-w-md">
        {app ? (
          <DetailBody
            key={app.appId}
            app={app}
            device={device}
            onDownload={(v) => onDownload(app, v)}
            onInstall={(v) => onInstall(app, v)}
            onOpenOnDevice={() => onOpenOnDevice(app)}
          />
        ) : null}
      </SheetContent>
    </Sheet>
  )
}
