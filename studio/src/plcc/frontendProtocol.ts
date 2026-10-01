// SPDX-License-Identifier: MPL-2.0

export type FrontendOp = 'check' | 'convert' | 'loadDevice' | 'validateDevice' | 'locateLadder' | 'catalog' | 'version'

export interface FrontendRequest {
  id: number
  op: FrontendOp
  args: unknown[]
}

export type FrontendResponse =
  | { id: number; ok: true; result: unknown; poisoned: string | null }
  | { id: number; ok: false; error: string; poisoned: string | null }
