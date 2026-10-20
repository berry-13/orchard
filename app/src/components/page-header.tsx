import { SidebarTrigger } from "@/components/ui/sidebar"
import { Separator } from "@/components/ui/separator"
import { TaskTray } from "@/components/task-tray"
import { overlayTitleBar } from "@/lib/platform"

/** Sticky top bar of every screen: sidebar toggle, title, actions, task tray. */
export function PageHeader({
  title,
  description,
  children,
}: {
  title: string
  description?: React.ReactNode
  children?: React.ReactNode
}) {
  return (
    <header
      data-tauri-drag-region={overlayTitleBar || undefined}
      className="sticky top-0 z-10 flex h-14 shrink-0 items-center gap-2 border-b bg-background/95 px-4 backdrop-blur supports-[backdrop-filter]:bg-background/80">
      <SidebarTrigger className="-ml-1" />
      <Separator orientation="vertical" className="mr-1 data-vertical:h-4 data-vertical:self-center" />
      <div className="flex min-w-0 flex-1 items-baseline gap-2">
        <h1 className="truncate text-sm font-semibold">{title}</h1>
        {description ? (
          <p className="hidden truncate text-sm text-muted-foreground md:block">{description}</p>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
      <TaskTray />
    </header>
  )
}

/** Scrollable content area under the header. */
export function PageBody({ className, children }: { className?: string; children: React.ReactNode }) {
  return <div className={className ?? "flex flex-1 flex-col gap-4 p-4 md:p-6"}>{children}</div>
}
