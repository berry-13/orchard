import { CircleAlert, CircleCheck, ListChecks, LoaderCircle, X } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { Progress } from "@/components/ui/progress"
import { formatBytes, formatRelative, stageLabel } from "@/lib/format"
import { taskPercent, useTasks, type Task } from "@/lib/tasks"
import { cn } from "@/lib/utils"

const BYTE_STAGES = new Set(["upload", "download", "export", "backup", "restore", "send"])

function detail(task: Task): string {
  if (task.status === "error") return task.message ?? "Failed"
  if (task.status === "done") return task.message ?? `Finished ${formatRelative(task.finishedAt)}`
  const stage = stageLabel(task.stage)
  if (task.total === null) return `${stage}...`
  if (BYTE_STAGES.has(task.stage)) return `${stage} ${formatBytes(task.done)} of ${formatBytes(task.total)}`
  if (task.total === 100) return `${stage} ${task.done}%`
  return `${stage} ${task.done} of ${task.total}`
}

function TaskRow({ task, onCancel }: { task: Task; onCancel: () => void }) {
  const percent = taskPercent(task)
  return (
    <li className="grid gap-2 px-3 py-2.5">
      <div className="flex items-start gap-2">
        <span className="mt-0.5" aria-hidden="true">
          {task.status === "running" ? (
            <LoaderCircle className="size-4 animate-spin text-muted-foreground motion-reduce:animate-none" />
          ) : task.status === "done" ? (
            <CircleCheck className="size-4 text-emerald-600 dark:text-emerald-500" />
          ) : (
            <CircleAlert className="size-4 text-destructive" />
          )}
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium" title={task.title}>
            {task.title}
          </p>
          <p
            className={cn(
              "truncate text-xs text-muted-foreground tabular-nums",
              task.status === "error" && "text-destructive",
            )}
          >
            {detail(task)}
          </p>
        </div>
        {task.status === "running" ? (
          <Button variant="ghost" size="icon-xs" onClick={onCancel} aria-label={`Cancel ${task.title}`}>
            <X />
          </Button>
        ) : null}
      </div>
      {task.status === "running" ? (
        <Progress
          value={percent}
          className="h-1"
          aria-label={`${task.title} progress`}
        />
      ) : null}
    </li>
  )
}

/** Header popover listing running and finished tasks. */
export function TaskTray() {
  const { tasks, cancel, clearFinished } = useTasks()
  const running = tasks.filter((t) => t.status === "running")
  const finished = tasks.length - running.length
  const overall =
    running.length === 0
      ? null
      : Math.round(running.reduce((sum, t) => sum + (taskPercent(t) ?? 0), 0) / running.length)

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="relative gap-1.5 text-muted-foreground"
        >
          {running.length ? (
            <LoaderCircle className="animate-spin motion-reduce:animate-none" />
          ) : (
            <ListChecks />
          )}
          <span className="tabular-nums max-sm:sr-only">
            {running.length ? (
              <>
                <span className="sr-only">Tasks, </span>
                {`${running.length} running${overall !== null ? `, ${overall}%` : ""}`}
              </>
            ) : (
              "Tasks"
            )}
          </span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 p-0">
        <div className="flex h-10 items-center justify-between border-b px-3">
          <h2 className="text-sm font-medium">Tasks</h2>
          {finished > 0 ? (
            <Button variant="ghost" size="xs" onClick={clearFinished}>
              Clear finished
            </Button>
          ) : null}
        </div>
        {tasks.length === 0 ? (
          <p className="px-3 py-8 text-center text-sm text-muted-foreground">
            Installs, downloads, transfers and backups show up here.
          </p>
        ) : (
          <ul className="max-h-96 divide-y overflow-y-auto" aria-label="Tasks">
            {tasks.map((task) => (
              <TaskRow key={task.id} task={task} onCancel={() => cancel(task.id)} />
            ))}
          </ul>
        )}
      </PopoverContent>
    </Popover>
  )
}

/** Visually hidden live region announcing task starts and completions. */
export function TaskAnnouncer() {
  const { tasks } = useTasks()
  const latest = tasks[0]
  const text = !latest
    ? ""
    : latest.status === "running"
      ? `${latest.title} started`
      : latest.status === "done"
        ? `${latest.title} finished`
        : `${latest.title} failed`
  return (
    <div className="sr-only" role="status" aria-live="polite">
      {text}
    </div>
  )
}
