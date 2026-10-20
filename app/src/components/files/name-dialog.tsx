import { useState } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

/** Single text field dialog used for new folder and rename. */
export function NameDialog({
  open,
  onOpenChange,
  title,
  description,
  label,
  initial,
  submitLabel,
  pending,
  error,
  onSubmit,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description: string
  label: string
  initial: string
  submitLabel: string
  pending: boolean
  error: string | null
  onSubmit: (name: string) => void
}) {
  const [name, setName] = useState(initial)
  const [prevOpen, setPrevOpen] = useState(open)
  if (open !== prevOpen) {
    setPrevOpen(open)
    if (open) setName(initial)
  }
  const trimmed = name.trim()
  const invalid = trimmed.includes("/") ? "Names cannot contain a slash" : null
  const message = invalid ?? error

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-sm">
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (trimmed && !invalid && trimmed !== initial) onSubmit(trimmed)
          }}
        >
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>{description}</DialogDescription>
          </DialogHeader>
          <div className="grid gap-2">
            <Label htmlFor="name-dialog-input">{label}</Label>
            <Input
              id="name-dialog-input"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onFocus={(e) => {
                const dot = e.target.value.lastIndexOf(".")
                e.target.setSelectionRange(0, dot > 0 ? dot : e.target.value.length)
              }}
              autoComplete="off"
              spellCheck={false}
              autoFocus
              aria-invalid={message ? true : undefined}
              aria-describedby={message ? "name-dialog-error" : undefined}
            />
            {message ? (
              <p id="name-dialog-error" className="text-sm text-destructive" role="alert">
                {message}
              </p>
            ) : null}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!trimmed || !!invalid || trimmed === initial || pending}>
              {pending ? "Saving..." : submitLabel}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
