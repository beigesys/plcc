// SPDX-License-Identifier: MPL-2.0
//
// Simulate with plcc's own output: compile the project to wasm32 in the
// browser (@plcc/plcc-compiler-wasm), link it, and hand the module to the
// simulator worker, which runs it with @plcc/plc-wasm. Until the first build
// is ready (the compiler downloading, compiling) and whenever it cannot be
// built, the preview engine runs instead, labelled as such.

import { create } from 'zustand'
import type { Device } from '@/devices/manifest'
import { toLadderJson, type Id, type Project } from '@/model'
import { compile, type CompileRequest } from '@/plcc/compiler'
import { plcc } from '@/plcc/frontend'
import { simSend, useLive } from './live'
import { toProblem, type Problem } from './problems'

export type BuildState = 'idle' | 'compiling' | 'running' | 'failed' | 'unavailable'

export interface SimBuildState {
  state: BuildState
  /** Problems of the last failed build. */
  problems: Problem[]
  message: string | null
  /** Compile + link time of the last build, ms (in the worker). */
  buildMs: number | null
  /** Where the fault that stopped the program is, in the model. */
  faultAt: { program?: string; routine?: string; rung?: Id; element?: Id } | null
  /** The project the running program was built from. */
  builtFrom: Project | null
}

export const useSimBuild = create<SimBuildState>(() => ({
  state: 'idle', problems: [], message: null, buildMs: null, faultAt: null, builtFrom: null,
}))

/** The build request for the simulator: wasm32, linked, the device's image sizes. */
export function simulatorRequest(project: Project, device: Pick<Device, 'target'>): CompileRequest {
  const img = device.target.image
  return {
    files: { 'project.json': toLadderJson(project) },
    entry: ['project.json'],
    target: 'wasm32-unknown-unknown',
    image: { I: img.I, Q: img.Q, M: img.M },
    opt_level: 2,
    link: true,
  }
}

let generation = 0

/** Builds the project and, when it builds, runs it in the simulator. The latest call wins. */
export async function buildForSimulator(project: Project, device: Pick<Device, 'target'>): Promise<void> {
  const g = ++generation
  useSimBuild.setState({ state: 'compiling', message: null })
  try {
    const t0 = performance.now()
    const r = await compile(simulatorRequest(project, device))
    if (g !== generation) return
    const ms = performance.now() - t0
    if (!r.ok || !r.module || !r.symbols) {
      const problems = r.diagnostics.map((d) => toProblem(d as Parameters<typeof toProblem>[0], 'compile'))
      const first = problems.find((p) => p.severity === 'error')
      useSimBuild.setState({
        state: 'failed',
        problems,
        buildMs: ms,
        message: first ? first.message : 'the build produced no module',
      })
      return
    }
    simSend({ type: 'program', module: r.module, symbols: r.symbols as never, project })
    useSimBuild.setState({ state: 'running', problems: [], buildMs: ms, faultAt: null, builtFrom: project })
  } catch (e) {
    if (g !== generation) return
    const message = e instanceof Error ? e.message : String(e)
    useSimBuild.setState({ state: /not available here|404/.test(message) ? 'unavailable' : 'failed', message })
  }
}

export function resetSimBuild() {
  generation++
  useSimBuild.setState({ state: 'idle', problems: [], message: null, buildMs: null, faultAt: null, builtFrom: null })
}

/** `file:line:col: POU` → line, col. */
export function parseSite(where: string): { line: number; col: number } | null {
  const m = /:(\d+):(\d+)(?::|$)/.exec(where)
  return m ? { line: Number(m[1]), col: Number(m[2]) } : null
}

// A fault names its site in the L5X the model was compiled as; plcc maps it back.
let lastFault: string | null = null
useLive.subscribe((s) => {
  const where = s.fault?.where ?? null
  if (where === lastFault) return
  lastFault = where
  const built = useSimBuild.getState().builtFrom
  const site = where ? parseSite(where) : null
  if (!site || !built) {
    useSimBuild.setState({ faultAt: null })
    return
  }
  void plcc
    .locateLadder(toLadderJson(built), site.line, site.col)
    .then((r) => {
      if (lastFault !== where) return
      useSimBuild.setState({ faultAt: r ? { program: r.pou, routine: r.routine, rung: r.rung, element: r.element } : null })
    })
    .catch(() => {})
})
