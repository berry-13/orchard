import { useQuery } from "@tanstack/react-query"
import { Archive } from "lucide-react"
import { useState } from "react"

import { BackupBrowser } from "@/components/backups/backup-browser"
import { BackupTable, CreateBackupDialog } from "@/components/backups/backup-list"
import { PageBody, PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import { errorMessage, listBackups, type BackupSummary } from "@/lib/api"
import { useDevices } from "@/lib/devices"

export default function BackupsScreen() {
  const { device } = useDevices()
  const backups = useQuery({ queryKey: ["backups"], queryFn: listBackups })
  const [open, setOpen] = useState<BackupSummary | null>(null)
  const [createOpen, setCreateOpen] = useState(false)

  const count = backups.data?.length
  const backupButton = device ? (
    <Button size="sm" onClick={() => setCreateOpen(true)}>
      <Archive aria-hidden="true" />
      Back up {device.name}
    </Button>
  ) : null

  return (
    <div className="flex h-full min-h-0 flex-col">
      <PageHeader
        title="Backups"
        description={open ? open.deviceName : count === undefined ? undefined : `${count} local ${count === 1 ? "backup" : "backups"}`}
      >
        {open ? null : backupButton}
      </PageHeader>
      {open ? (
        <BackupBrowser key={open.id} backup={open} device={device} onBack={() => setOpen(null)} />
      ) : (
        <PageBody>
          {backups.isError ? (
            <p className="text-sm text-destructive" role="alert">
              {errorMessage(backups.error)}
            </p>
          ) : (
            <BackupTable backups={backups.data} loading={backups.isPending} onOpen={setOpen} />
          )}
        </PageBody>
      )}
      {device ? <CreateBackupDialog device={device} open={createOpen} onOpenChange={setCreateOpen} /> : null}
    </div>
  )
}
