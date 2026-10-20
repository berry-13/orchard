import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ChevronDown, LogOut, Search, SearchX, ShoppingBag } from "lucide-react"
import { useState } from "react"
import { toast } from "sonner"

import { EmptyState } from "@/components/empty-state"
import { PageBody, PageHeader } from "@/components/page-header"
import { AppDetailSheet } from "@/components/store/app-detail"
import { countryName } from "@/components/store/countries"
import { Rating } from "@/components/store/rating"
import { SignInCard } from "@/components/store/sign-in"
import { useStoreActions } from "@/components/store/store-actions"
import { Avatar, AvatarFallback } from "@/components/ui/avatar"
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
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group"
import { Skeleton } from "@/components/ui/skeleton"
import {
  errorMessage,
  isOrchardError,
  openStorePage,
  storeAccount,
  storeSearch,
  storeSignOut,
  type StoreAccount,
  type StoreApp,
} from "@/lib/api"
import { useDevices } from "@/lib/devices"

const DEFAULT_TERM = "productivity"

function initials(name: string) {
  return (
    name
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((p) => p.charAt(0).toUpperCase())
      .join("") || "?"
  )
}

function AccountMenu({ account }: { account: StoreAccount }) {
  const queryClient = useQueryClient()
  const signOut = useMutation({
    mutationFn: storeSignOut,
    onSuccess: () => {
      queryClient.setQueryData(["storeAccount"], null)
      queryClient.removeQueries({ queryKey: ["storeSearch"] })
      toast.success("Signed out")
    },
    onError: (e) => toast.error("Could not sign out", { description: errorMessage(e) }),
  })
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className="gap-2 px-1.5" aria-label={`Account: ${account.email}`}>
          <Avatar className="size-6">
            <AvatarFallback className="text-[10px] font-medium">{initials(account.name)}</AvatarFallback>
          </Avatar>
          <span className="hidden max-w-40 truncate md:inline">{account.email}</span>
          <ChevronDown className="text-muted-foreground" aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64">
        <DropdownMenuLabel className="grid gap-0.5 font-normal">
          <span className="truncate text-sm font-medium text-foreground capitalize">{account.name}</span>
          <span className="truncate text-xs text-muted-foreground">{account.email}</span>
          <span className="truncate text-xs text-muted-foreground">Store: {countryName(account.country)}</span>
          <span className="truncate text-xs text-muted-foreground">
            {account.backend === "ipatool" ? "Signed in via ipatool" : "Signed in with the built-in client"}
          </span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => signOut.mutate()} disabled={signOut.isPending}>
          <LogOut aria-hidden="true" />
          Sign out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function ResultCard({ app, onOpen }: { app: StoreApp; onOpen: () => void }) {
  return (
    <li>
      <button
        type="button"
        onClick={onOpen}
        className="group flex h-full w-full items-start gap-3 rounded-lg border bg-card p-3 text-left transition-colors duration-150 outline-none hover:bg-muted/50 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
        aria-label={`${app.name} by ${app.seller}, ${app.formattedPrice}`}
      >
        <img
          src={app.artworkUrl}
          alt=""
          loading="lazy"
          className="size-14 shrink-0 rounded-[22%] border"
          draggable={false}
        />
        <div className="grid min-w-0 flex-1 gap-0.5">
          <div className="flex items-start justify-between gap-2">
            <span className="line-clamp-2 text-sm leading-snug font-medium">{app.name}</span>
            <Badge variant={app.price > 0 ? "default" : "secondary"} className="shrink-0 tabular-nums">
              {app.price > 0 ? app.formattedPrice : "Get"}
            </Badge>
          </div>
          <span className="truncate text-xs text-muted-foreground">{app.seller}</span>
          <div className="mt-1 flex items-center justify-between gap-2">
            <Rating value={app.averageRating} count={app.ratingCount} />
            <span className="truncate text-xs text-muted-foreground">{app.genre}</span>
          </div>
        </div>
      </button>
    </li>
  )
}

function ResultsSkeleton() {
  return (
    <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3" aria-busy="true" aria-label="Searching">
      {Array.from({ length: 9 }, (_, i) => (
        <li key={i} className="flex items-start gap-3 rounded-lg border p-3">
          <Skeleton className="size-14 rounded-[22%]" />
          <div className="grid flex-1 gap-2 pt-1">
            <Skeleton className="h-3.5 w-3/4" />
            <Skeleton className="h-3 w-1/2" />
            <Skeleton className="h-3 w-2/3" />
          </div>
        </li>
      ))}
    </ul>
  )
}

function StoreBrowser({ account }: { account: StoreAccount }) {
  const { device } = useDevices()
  const queryClient = useQueryClient()
  const { download, install, licenseDialog } = useStoreActions()
  const [input, setInput] = useState("")
  const [term, setTerm] = useState(DEFAULT_TERM)
  const [selected, setSelected] = useState<StoreApp | null>(null)
  const [sheetOpen, setSheetOpen] = useState(false)

  const results = useQuery({
    queryKey: ["storeSearch", term, account.country],
    queryFn: () => storeSearch(term, account.country),
    staleTime: 5 * 60_000,
    retry: (count, e) => !isOrchardError(e, "authSignedOut") && count < 1,
  })

  const authLost =
    isOrchardError(results.error, "authSignedOut") || isOrchardError(results.error, "authSessionExpired")

  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    const next = input.trim()
    if (next) setTerm(next)
  }

  return (
    <>
      <PageHeader title="App Store" description={`${countryName(account.country)} storefront`}>
        <AccountMenu account={account} />
      </PageHeader>
      <PageBody>
        <form role="search" onSubmit={submit} className="flex gap-2">
          <InputGroup className="max-w-md">
            <InputGroupAddon>
              <Search aria-hidden="true" />
            </InputGroupAddon>
            <InputGroupInput
              type="search"
              placeholder="Search apps, developers or categories"
              aria-label="Search the App Store"
              value={input}
              onChange={(e) => setInput(e.target.value)}
            />
            <InputGroupAddon align="inline-end">
              <InputGroupButton type="submit" variant="secondary" disabled={!input.trim()}>
                Search
              </InputGroupButton>
            </InputGroupAddon>
          </InputGroup>
        </form>

        <div className="flex items-baseline justify-between gap-2">
          <h2 className="text-sm font-medium">
            {term === DEFAULT_TERM ? "Suggested" : <>Results for &ldquo;{term}&rdquo;</>}
          </h2>
          {results.data ? (
            <p className="text-xs text-muted-foreground tabular-nums">{results.data.length} apps</p>
          ) : null}
        </div>

        {results.isPending ? (
          <ResultsSkeleton />
        ) : results.isError ? (
          <EmptyState
            icon={ShoppingBag}
            title="Search failed"
            description={errorMessage(results.error)}
            action={
              authLost ? (
                <Button size="sm" onClick={() => queryClient.setQueryData(["storeAccount"], null)}>
                  Sign in again
                </Button>
              ) : (
                <Button variant="outline" size="sm" onClick={() => void results.refetch()}>
                  Try again
                </Button>
              )
            }
          />
        ) : results.data.length === 0 ? (
          <EmptyState icon={SearchX} title="No apps found" description="Try a different name or a broader term." />
        ) : (
          <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3 animate-in fade-in-0 duration-200" aria-label="Search results">
            {results.data.map((app) => (
              <ResultCard
                key={app.appId}
                app={app}
                onOpen={() => {
                  setSelected(app)
                  setSheetOpen(true)
                }}
              />
            ))}
          </ul>
        )}
        <p className="sr-only" role="status" aria-live="polite">
          {results.isPending ? "Searching" : results.data ? `${results.data.length} results` : ""}
        </p>
      </PageBody>

      <AppDetailSheet
        app={selected}
        open={sheetOpen}
        device={device}
        onOpenChange={setSheetOpen}
        onDownload={(app, versionId) => void download(app, versionId)}
        onInstall={(app, versionId) => {
          if (!device) return
          setSheetOpen(false)
          install(app, versionId, device)
        }}
        onOpenOnDevice={(app) => {
          if (!device) return
          openStorePage(device.udid, app.appId).then(
            () =>
              toast.success(`${app.name} is open on ${device.name}`, {
                description: "Tap Get in the App Store there to install it with that iPhone's Apple ID.",
              }),
            (e: unknown) =>
              toast.error(`Could not open the App Store on ${device.name}`, { description: errorMessage(e) }),
          )
        }}
      />
      {licenseDialog}
    </>
  )
}

export default function AppStoreScreen() {
  const account = useQuery({ queryKey: ["storeAccount"], queryFn: storeAccount, staleTime: Infinity })

  if (account.data) return <StoreBrowser account={account.data} />

  return (
    <>
      <PageHeader title="App Store" />
      <PageBody>
        {account.isPending ? (
          <div className="flex flex-1 items-center justify-center py-8">
            <Skeleton className="h-[26rem] w-full max-w-sm" aria-label="Loading account" />
          </div>
        ) : account.isError ? (
          <EmptyState
            icon={ShoppingBag}
            title="Could not reach the App Store"
            description={errorMessage(account.error)}
            action={
              <Button variant="outline" size="sm" onClick={() => void account.refetch()}>
                Try again
              </Button>
            }
          />
        ) : (
          <SignInCard />
        )}
      </PageBody>
    </>
  )
}
