// SPDX-License-Identifier: MPL-2.0

/** The plcc mark: two rails and a contact, in the power color. */
export function Logo({ size = 22 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true">
      <rect width="32" height="32" rx="7" fill="var(--surface-2)" stroke="var(--border)" />
      <g stroke="var(--power)" strokeWidth="2.4" strokeLinecap="round" fill="none">
        <path d="M6 7v18M26 7v18M6 16h6M20 16h6" />
        <path d="M12 11v10M20 11v10" />
      </g>
    </svg>
  )
}
