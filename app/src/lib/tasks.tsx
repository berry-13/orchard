import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react"
import { toast } from "sonner"

import {
  cancelTask,
  currentBackend,
  errorMessage,
  EVENTS,
  inTauri,
  isOrchardError,
  newTaskId,
  revealPath,
  subscribe,
  type TaskEvent,
} from "@/lib/api"

export type TaskKind = "install" | "download" | "transfer" | "backup" | "restore" | "export"

export interface Task {
  id: string
  kind: TaskKind
  title: string
  stage: string
  done: number
  total: number | null
  status: "running" | "done" | "error"
  message: string | null
  startedAt: number
  finishedAt: number | null
}

interface StartOptions<T> {
  kind: TaskKind
  title: string
  /** Runs the backend command with the generated task id. */
  run: (taskId: string) => Promise<T>
  /** Toast text on success; false to stay quiet. */
  successMessage?: string | false
  /** Where the results were written; the success toast offers to show it (or download it, on a server). */
  output?: string
  /** Handles an error instead of the default error toast; return true when handled. */
  onError?: (error: unknown) => boolean
}

interface TaskContextValue {
  tasks: Task[]
  start: <T>(options: StartOptions<T>) => Promise<T | undefined>
  cancel: (id: string) => void
  clearFinished: () => void
}

const TaskContext = createContext<TaskContextValue | null>(null)

export function TaskProvider({ children }: { children: React.ReactNode }) {
  const [tasks, setTasks] = useState<Task[]>([])
  const patch = useCallback((id: string, update: Partial<Task>) => {
    setTasks((list) => list.map((t) => (t.id === id ? { ...t, ...update } : t)))
  }, [])

  useEffect(() => {
    const unlisten = subscribe(EVENTS.task, (e: TaskEvent) => {
      setTasks((list) =>
        list.map((t) =>
          t.id !== e.taskId || t.status !== "running"
            ? t
            : {
                ...t,
                stage: e.stage,
                done: e.done,
                total: e.total,
                message: e.message ?? t.message,
              },
        ),
      )
    })
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [])

  const start = useCallback(
    async <T,>({ kind, title, run, successMessage, output, onError }: StartOptions<T>) => {
      const id = newTaskId()
      setTasks((list) => [
        {
          id,
          kind,
          title,
          stage: "prepare",
          done: 0,
          total: null,
          status: "running",
          message: null,
          startedAt: Date.now(),
          finishedAt: null,
        },
        ...list,
      ])
      try {
        const result = await run(id)
        patch(id, { status: "done", finishedAt: Date.now() })
        if (successMessage !== false) {
          const remote = currentBackend() === "remote"
          toast.success(successMessage ?? `${title} finished`, {
            action:
              output && (remote || inTauri)
                ? { label: remote ? "Download" : "Show", onClick: () => void revealPath(output) }
                : undefined,
          })
        }
        return result
      } catch (e) {
        const message = isOrchardError(e, "cancelled") ? "Cancelled" : errorMessage(e)
        patch(id, { status: "error", message, finishedAt: Date.now() })
        if (onError?.(e)) {
          setTasks((list) => list.filter((t) => t.id !== id))
          return undefined
        }
        if (!isOrchardError(e, "cancelled")) toast.error(title, { description: message })
        return undefined
      }
    },
    [patch],
  )

  const cancel = useCallback((id: string) => {
    void cancelTask(id).catch(() => undefined)
  }, [])

  const clearFinished = useCallback(() => {
    setTasks((list) => list.filter((t) => t.status === "running"))
  }, [])

  const value = useMemo(() => ({ tasks, start, cancel, clearFinished }), [tasks, start, cancel, clearFinished])
  return <TaskContext.Provider value={value}>{children}</TaskContext.Provider>
}

export function useTasks() {
  const ctx = useContext(TaskContext)
  if (!ctx) throw new Error("useTasks must be used inside TaskProvider")
  return ctx
}

/** 0 to 100, or null when the task has no known total. */
export function taskPercent(task: Pick<Task, "done" | "total" | "status">): number | null {
  if (task.status === "done") return 100
  if (!task.total) return null
  return Math.min(100, Math.round((task.done / task.total) * 100))
}
