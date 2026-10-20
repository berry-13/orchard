import { useQueryClient } from "@tanstack/react-query"
import { useState } from "react"
import { toast } from "sonner"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import {
  errorMessage,
  getSettings,
  isOrchardError,
  pickFolder,
  revealPath,
  storeDownload,
  storeInstall,
  storePurchase,
  type Device,
  type PurchaseConfirmation,
  type StoreApp,
} from "@/lib/api"
import { useTasks } from "@/lib/tasks"

interface LicensePrompt {
  app: StoreApp
  retry: () => void
  /** Apple's price confirmation, once a paid purchase asked for it. */
  confirmation?: PurchaseConfirmation
}

/**
 * Download and install actions for store apps. A `licenseRequired` failure
 * offers to get (free) or buy (paid, after Apple's confirmation) the app and
 * then retries the same action.
 */
export function useStoreActions() {
  const tasks = useTasks()
  const queryClient = useQueryClient()
  const [prompt, setPrompt] = useState<LicensePrompt | null>(null)
  const [purchasing, setPurchasing] = useState(false)

  const handleError = (app: StoreApp, retry: () => void) => (e: unknown) => {
    if (isOrchardError(e, "licenseRequired")) {
      setPrompt({ app, retry })
      return true
    }
    if (isOrchardError(e, "authSignedOut") || isOrchardError(e, "authSessionExpired")) {
      toast.error("Sign in again", { description: errorMessage(e) })
      void queryClient.invalidateQueries({ queryKey: ["storeAccount"] })
      return true
    }
    return false
  }

  const download = async (app: StoreApp, versionId: string | null) => {
    const settings = await getSettings().catch(() => null)
    const destDir = settings?.downloadDir ?? (await pickFolder("Save IPA to"))
    if (!destDir) return
    const run = () => {
      void tasks
        .start({
          kind: "download",
          title: `Download ${app.name}`,
          successMessage: false,
          run: (taskId) => storeDownload(taskId, app.appId, versionId, destDir),
          onError: handleError(app, run),
        })
        .then((path) => {
          if (!path) return
          toast.success(`${app.name} downloaded`, {
            description: path,
            action: { label: "Show", onClick: () => void revealPath(path) },
          })
        })
    }
    run()
  }

  const install = (app: StoreApp, versionId: string | null, device: Device) => {
    const run = () => {
      void tasks.start({
        kind: "install",
        title: `Install ${app.name}`,
        successMessage: `${app.name} installed on ${device.name}`,
        run: (taskId) => storeInstall(taskId, device.udid, app.appId, versionId),
        onError: handleError(app, run),
      }).then(() => queryClient.invalidateQueries({ queryKey: ["apps", device.udid] }))
    }
    run()
  }

  const get = async () => {
    if (!prompt) return
    const { app, retry, confirmation } = prompt
    setPurchasing(true)
    try {
      const result = await storePurchase(app.appId, app.price, confirmation ?? null)
      if (result.status === "needsConfirmation") {
        if (confirmation) throw new Error("Apple asked for another confirmation. Try again later.")
        const { message, okLabel, buyParams } = result
        setPrompt({ app, retry, confirmation: { message, okLabel, buyParams } })
        return
      }
      toast.success(app.price > 0 ? `${app.name} bought` : `${app.name} added to your purchases`)
      setPrompt(null)
      retry()
    } catch (e) {
      toast.error(`Could not ${app.price > 0 ? "buy" : "get"} ${app.name}`, {
        description: errorMessage(e),
      })
      setPrompt(null)
    } finally {
      setPurchasing(false)
    }
  }

  const paid = (prompt?.app.price ?? 0) > 0
  const title = prompt?.confirmation
    ? "Confirm purchase"
    : `${paid ? "Buy" : "Get"} ${prompt?.app.name ?? "app"}?`
  const description = prompt?.confirmation
    ? prompt.confirmation.message
    : paid
      ? `This Apple ID does not own ${prompt?.app.name}. Buying it charges ${prompt?.app.formattedPrice} to the payment method on your Apple ID; Apple asks you to confirm the price first.`
      : `This Apple ID does not own ${prompt?.app.name ?? "this app"} yet. Getting it adds the free app to your purchases, then Orchard retries.`
  const actionLabel = purchasing
    ? paid
      ? "Buying..."
      : "Getting..."
    : (prompt?.confirmation?.okLabel ?? (paid ? `Buy for ${prompt?.app.formattedPrice}` : "Get"))

  const licenseDialog = (
    <AlertDialog open={prompt !== null} onOpenChange={(open) => !open && !purchasing && setPrompt(null)}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={purchasing}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            disabled={purchasing}
            onClick={(e) => {
              e.preventDefault()
              void get()
            }}
          >
            {actionLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )

  return { download, install, licenseDialog }
}
