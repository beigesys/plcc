// SPDX-License-Identifier: MPL-2.0

export type DotState = 'power' | 'idle' | 'alarm' | 'fault' | 'unknown'

const LABEL: Record<DotState, string> = {
  power: 'on',
  idle: 'off',
  alarm: 'alarm',
  fault: 'fault',
  unknown: 'unknown',
}

/** A state indicator. Filled with the state color; hollow when off or unknown. */
export function StateDot({ state, label }: { state: DotState; label?: string }) {
  const color =
    state === 'power' ? 'var(--power)' : state === 'alarm' ? 'var(--alarm)' : state === 'fault' ? 'var(--fault)' : 'var(--line-idle)'
  const filled = state === 'power' || state === 'alarm' || state === 'fault'
  return (
    <span
      role="img"
      aria-label={label ?? LABEL[state]}
      className="inline-block size-2.5 shrink-0 rounded-full"
      style={{
        background: filled ? color : 'transparent',
        boxShadow: `inset 0 0 0 1.5px ${color}`,
        opacity: state === 'unknown' ? 0.5 : 1,
      }}
    />
  )
}
