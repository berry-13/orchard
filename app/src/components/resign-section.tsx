import { useMutation, useQuery } from "@tanstack/react-query"
import { BadgeCheck, CircleCheck, CircleX, FileKey, KeyRound, OctagonX, TriangleAlert } from "lucide-react"
import { useState } from "react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  errorMessage,
  getSettings,
  loadProvisioningProfile,
  loadSigningIdentity,
  pickP12,
  pickProfile,
  type ProfileInfo,
  type ResignIssue,
  type SigningIdentityInfo,
} from "@/lib/api"
import { formatDate } from "@/lib/format"
import { cn } from "@/lib/utils"

export interface ResignForm {
  identity: SigningIdentityInfo | null
  profile: ProfileInfo | null
  bundleId: string
  displayName: string
}

export const EMPTY_RESIGN: ResignForm = { identity: null, profile: null, bundleId: "", displayName: "" }

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path

/** Why an encrypted IPA cannot be re-signed, shown as a blocking error. */
export const ENCRYPTED_MESSAGE =
  "This IPA is FairPlay encrypted, as every App Store download is. Encrypted apps only run with Apple's own signature, so they cannot be re-signed. Use a decrypted IPA instead."

export function issueMessage(issue: ResignIssue, deviceName: string | null): string {
  switch (issue.kind) {
    case "encrypted":
      return ENCRYPTED_MESSAGE
    case "certificateExpired":
      return `The certificate expired on ${formatDate(issue.expiredAt)}.`
    case "profileExpired":
      return `The provisioning profile expired on ${formatDate(issue.expiredAt)}.`
    case "teamMismatch":
      return `The certificate (team ${issue.certificate ?? "unknown"}) and the profile (team ${issue.profile}) belong to different teams.`
    case "certificateNotInProfile":
      return "The certificate is not one of the profile's developer certificates."
    case "bundleIdNotMatched":
      return `The profile covers ${issue.pattern}, not ${issue.bundleId}. Set a new bundle ID that matches it.`
    case "deviceNotProvisioned":
      return `${deviceName ?? "This device"} is not in the profile, so the app will not launch on it.`
  }
}

/** Errors and warnings from the re-sign check, encrypted first. */
export function ResignIssues({ issues, deviceName }: { issues: ResignIssue[]; deviceName: string | null }) {
  if (issues.length === 0) return null
  return (
    <ul className="grid gap-2" aria-label="Re-signing problems">
      {issues.map((issue) => {
        const blocking = issue.kind === "encrypted"
        const Icon = blocking ? OctagonX : TriangleAlert
        return (
          <li
            key={issue.kind}
            className={cn(
              "flex gap-2 rounded-lg border px-3 py-2 text-sm",
              blocking
                ? "border-destructive/30 bg-destructive/5 text-destructive"
                : "border-amber-500/30 bg-amber-500/5 text-amber-900 dark:text-amber-200",
            )}
          >
            <Icon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <span role={blocking ? "alert" : undefined}>
              <span className="sr-only">{blocking ? "Error: " : "Warning: "}</span>
              {issueMessage(issue, deviceName)}
            </span>
          </li>
        )
      })}
    </ul>
  )
}

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[6.5rem_minmax(0,1fr)] items-baseline gap-2 py-1.5 text-xs">
      <dt className="truncate text-muted-foreground" title={label}>
        {label}
      </dt>
      <dd className="min-w-0 truncate">{children}</dd>
    </div>
  )
}

function IdentityPicker({
  value,
  onChange,
  remembered,
}: {
  value: SigningIdentityInfo | null
  onChange: (identity: SigningIdentityInfo | null) => void
  remembered: string | null
}) {
  const [path, setPath] = useState<string | null>(null)
  const [password, setPassword] = useState("")
  const current = path ?? remembered
  const load = useMutation({
    mutationFn: () => loadSigningIdentity(current!, password),
    onSuccess: (identity) => {
      setPassword("")
      onChange(identity)
    },
  })

  if (value) {
    return (
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-1 rounded-lg border p-3">
        <div className="flex items-center gap-2">
          <BadgeCheck className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <p className="min-w-0 flex-1 truncate text-sm font-medium" title={value.commonName ?? undefined}>
            {value.commonName ?? fileName(value.path)}
          </p>
          <Button variant="ghost" size="xs" onClick={() => onChange(null)}>
            Change
          </Button>
        </div>
        <dl className="pl-6">
          <Fact label="Team">{value.teamId ?? "Unknown"}</Fact>
          <Fact label="Expires">{formatDate(value.expiresAt)}</Fact>
        </dl>
      </div>
    )
  }

  return (
    <form
      className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 rounded-lg border p-3"
      onSubmit={(e) => {
        e.preventDefault()
        if (current) load.mutate()
      }}
    >
      <div className="flex items-center gap-2">
        <KeyRound className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <p className="min-w-0 flex-1 truncate text-sm" title={current ?? undefined}>
          {current ? fileName(current) : <span className="text-muted-foreground">No certificate chosen</span>}
        </p>
        <Button
          type="button"
          variant="outline"
          size="xs"
          onClick={() =>
            void pickP12(current).then((p) => {
              if (p) {
                setPath(p)
                load.reset()
              }
            })
          }
        >
          Choose .p12...
        </Button>
      </div>
      {current ? (
        <div className="grid gap-1.5">
          <Label htmlFor="p12-password" className="text-xs font-normal text-muted-foreground">
            Certificate password (used once, not saved)
          </Label>
          <div className="flex gap-2">
            <Input
              id="p12-password"
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              aria-invalid={load.isError || undefined}
              aria-describedby={load.isError ? "p12-error" : undefined}
              className="h-7"
            />
            <Button type="submit" size="sm" disabled={load.isPending}>
              {load.isPending ? "Unlocking..." : "Unlock"}
            </Button>
          </div>
          {load.isError ? (
            <p id="p12-error" className="text-xs text-destructive" role="alert">
              {errorMessage(load.error)}
            </p>
          ) : null}
        </div>
      ) : null}
    </form>
  )
}

function ProfilePicker({
  value,
  onChange,
  remembered,
  udid,
  deviceName,
}: {
  value: ProfileInfo | null
  onChange: (profile: ProfileInfo | null) => void
  remembered: string | null
  udid: string | null
  deviceName: string | null
}) {
  const load = useMutation({ mutationFn: (path: string) => loadProvisioningProfile(path), onSuccess: onChange })

  if (value) {
    const includes =
      udid === null
        ? null
        : value.provisionsAllDevices || value.devices.some((d) => d.toLowerCase() === udid.toLowerCase())
    return (
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-1 rounded-lg border p-3">
        <div className="flex items-center gap-2">
          <FileKey className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <p className="min-w-0 flex-1 truncate text-sm font-medium" title={value.name}>
            {value.name}
          </p>
          <Button variant="ghost" size="xs" onClick={() => onChange(null)}>
            Change
          </Button>
        </div>
        <dl className="pl-6">
          <Fact label="Team">
            {value.teamName ? `${value.teamName} (${value.teamId})` : value.teamId}
          </Fact>
          <Fact label="App ID">
            <span className="font-mono">{value.bundleIdPattern}</span>{" "}
            <Badge variant="outline" className="ml-1 h-4 px-1.5 text-[10px] font-normal text-muted-foreground">
              {value.wildcard ? "Wildcard" : "Explicit"}
            </Badge>
          </Fact>
          <Fact label="Expires">{formatDate(value.expiresAt)}</Fact>
          <Fact label="Devices">
            {value.provisionsAllDevices ? "All devices (enterprise)" : `${value.devices.length} provisioned`}
          </Fact>
          {includes !== null ? (
            <Fact label={deviceName ?? "This device"}>
              <span className={cn("inline-flex items-center gap-1", !includes && "text-destructive")}>
                {includes ? (
                  <CircleCheck className="size-3.5 text-emerald-600 dark:text-emerald-500" aria-hidden="true" />
                ) : (
                  <CircleX className="size-3.5" aria-hidden="true" />
                )}
                {includes ? "Included" : "Not included"}
              </span>
            </Fact>
          ) : null}
        </dl>
      </div>
    )
  }

  return (
    <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 rounded-lg border p-3">
      <div className="flex items-center gap-2">
        <FileKey className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <p className="min-w-0 flex-1 truncate text-sm text-muted-foreground">No provisioning profile chosen</p>
        {remembered ? (
          <Button
            variant="ghost"
            size="xs"
            onClick={() => load.mutate(remembered)}
            disabled={load.isPending}
            title={remembered}
          >
            Use last
          </Button>
        ) : null}
        <Button
          variant="outline"
          size="xs"
          disabled={load.isPending}
          onClick={() => void pickProfile(remembered).then((p) => p && load.mutate(p))}
        >
          {load.isPending ? "Reading..." : "Choose profile..."}
        </Button>
      </div>
      {load.isError ? (
        <p className="text-xs text-destructive" role="alert">
          {errorMessage(load.error)}
        </p>
      ) : null}
    </div>
  )
}

/** Certificate, profile and optional changes for re-signing an IPA. */
export function ResignFields({
  value,
  onChange,
  currentBundleId,
  currentName,
  udid,
  deviceName,
}: {
  value: ResignForm
  onChange: (next: ResignForm) => void
  currentBundleId: string
  currentName: string
  udid: string | null
  deviceName: string | null
}) {
  const settings = useQuery({ queryKey: ["settings"], queryFn: getSettings })
  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-3">
      <div className="grid gap-1.5">
        <p className="text-xs font-medium text-muted-foreground">Signing certificate</p>
        <IdentityPicker
          value={value.identity}
          onChange={(identity) => onChange({ ...value, identity })}
          remembered={settings.data?.signingP12Path ?? null}
        />
      </div>
      <div className="grid gap-1.5">
        <p className="text-xs font-medium text-muted-foreground">Provisioning profile</p>
        <ProfilePicker
          value={value.profile}
          onChange={(profile) => onChange({ ...value, profile })}
          remembered={settings.data?.signingProfilePath ?? null}
          udid={udid}
          deviceName={deviceName}
        />
      </div>
      <div className="grid grid-cols-[minmax(0,1fr)] gap-3">
        <div className="grid gap-1.5">
          <Label htmlFor="resign-bundle" className="text-xs font-medium text-muted-foreground">
            New bundle ID (optional)
          </Label>
          <Input
            id="resign-bundle"
            placeholder={currentBundleId}
            value={value.bundleId}
            onChange={(e) => onChange({ ...value, bundleId: e.target.value })}
            autoComplete="off"
            spellCheck={false}
            className="h-8 font-mono text-xs"
          />
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="resign-name" className="text-xs font-medium text-muted-foreground">
            New display name (optional)
          </Label>
          <Input
            id="resign-name"
            placeholder={currentName}
            value={value.displayName}
            onChange={(e) => onChange({ ...value, displayName: e.target.value })}
            autoComplete="off"
            className="h-8"
          />
        </div>
      </div>
    </div>
  )
}
