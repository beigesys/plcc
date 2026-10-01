// SPDX-License-Identifier: MPL-2.0
// Download: compile → program image → (click) check the board, reboot to
// DFU, write and verify the program slot, start the runtime.

import { useEffect } from 'react'
import { Check, Circle, CircleX, Loader2, Usb } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { primaryDevice } from '@/devices/project'
import { useCompiler } from '@/plcc/compiler'
import { browserIo, buildImage, continueWithUsb, flashImage, resetDownload, useDownload, type StepState } from '@/state/download'
import { useEditor } from '@/state/editor'

function Icon({ state }: { state: StepState }) {
  if (state === 'ok') return <Check className="size-4 text-power-text" aria-label="done" />
  if (state === 'failed') return <CircleX className="size-4 text-fault" aria-label="failed" />
  if (state === 'active') return <Loader2 className="size-4 animate-spin text-text" aria-label="running" />
  return <Circle className="size-4 text-text-muted" aria-label="pending" />
}

const hex = (n: number) => `0x${n.toString(16).toUpperCase().padStart(8, '0')}`

export function DownloadDialog() {
  const dialog = useEditor((s) => s.dialog)
  const setDialog = useEditor((s) => s.setDialog)
  const project = useEditor((s) => s.project)
  const dl = useDownload()
  const compiler = useCompiler()
  const open = dialog?.kind === 'download'
  const device = primaryDevice(project)
  const ref = project.devices[0]
  const manifestText = ref ? project.deviceFiles[ref.manifest] : undefined
  const slot = device.flash?.program

  // Building touches no hardware: start it when the dialog opens.
  useEffect(() => {
    if (!open) return
    const { project: p } = useEditor.getState()
    const d = primaryDevice(p)
    const text = p.devices[0] ? p.deviceFiles[p.devices[0].manifest] : undefined
    if (text) void buildImage(p, d, text)
  }, [open])

  const flash = async () => {
    if (!dl.image) return
    try {
      await flashImage(device, dl.image, await browserIo())
    } catch (e) {
      useDownload.setState({ error: e instanceof Error ? e.message : String(e) })
    }
  }

  const close = () => {
    if (dl.running) return
    setDialog(null)
    resetDownload()
  }
  const p = dl.progress
  const loading = compiler.status === 'loading' && compiler.progress
  const flashed = dl.steps.find((s) => s.id === 'done')?.state === 'ok'

  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>Download to {ref?.name ?? device.device.name}</DialogTitle>
          <DialogDescription>
            {slot ? (
              <>
                plcc compiles the project for {device.device.name} ({device.target.triple}) here in the browser and links it into a program image for
                the device's program slot ({hex(slot.address)}, {slot.max_size / 1024} KiB). Only the slot is written; the runtime and the bootloader are
                never touched. The device must run the program-image runtime (flashed once; runtimes/arduino-opta).
              </>
            ) : (
              <>{device.device.name} has no program slot in its manifest, so programs cannot be downloaded to it.</>
            )}
          </DialogDescription>
        </DialogHeader>
        {loading && compiler.progress && (
          <p className="text-dense text-text-muted" role="status">
            {compiler.progress.phase === 'download'
              ? `Downloading plcc's compiler: ${Math.round((100 * compiler.progress.loaded) / Math.max(1, compiler.progress.total))}% of ${(compiler.progress.total / 1e6).toFixed(1)} MB (once; it is cached)`
              : "Starting plcc's compiler…"}
          </p>
        )}
        <ol className="space-y-1.5" aria-label="Download steps" data-testid="download-steps">
          {dl.steps.map((s) => (
            <li key={s.id} data-step={s.id} data-state={s.state} className="flex items-start gap-2 text-dense">
              <Icon state={s.state} />
              <span className="min-w-0 flex-1">
                <span className={s.state === 'pending' ? 'text-text-muted' : ''}>{s.title}</span>
                {s.detail && <span className={`block text-[11px] ${s.state === 'failed' ? 'text-fault' : 'text-text-muted'}`}>{s.detail}</span>}
                {s.id === 'flash' && s.state === 'active' && p && (
                  <span className="mt-1 block">
                    <span className="mb-0.5 block text-[11px] text-text-muted">
                      {p.phase} {p.done}/{p.total}
                    </span>
                    <progress className="h-1.5 w-full" max={p.total || 1} value={p.done} aria-label={`${p.phase} progress`} />
                  </span>
                )}
              </span>
            </li>
          ))}
        </ol>
        {dl.problems.length > 0 && (
          <ul aria-label="Compile errors" className="max-h-40 space-y-1 overflow-y-auto rounded-lg border border-line bg-bg p-2 text-dense">
            {dl.problems.map((x, i) => (
              <li key={i} className={x.severity === 'error' ? 'text-fault' : 'text-alarm'}>
                {x.message}
              </li>
            ))}
          </ul>
        )}
        {dl.error && !dl.problems.length && (
          <p role="alert" className="text-dense text-fault">
            {dl.error}
          </p>
        )}
        <div className="flex items-center justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={close} disabled={dl.running}>
            {flashed ? 'Close' : 'Cancel'}
          </Button>
          {dl.waitingFor === 'usb' ? (
            <Button size="sm" onClick={() => void continueWithUsb()}>
              <Usb /> Allow access to the bootloader…
            </Button>
          ) : (
            <Button size="sm" disabled={!dl.image || dl.running || flashed || !manifestText} onClick={() => void flash()}>
              <Usb /> Flash over USB…
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
