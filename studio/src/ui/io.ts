// SPDX-License-Identifier: MPL-2.0
import { parseAddress, ProcessImage, typesForSize, type Tag } from '@/model'
import type { IoPoint } from '@/devices/manifest'
import type { LiveState } from '@/state/live'
import type { Scalar } from '@/runtime/messages'
import type { DotState } from './StateDot'

/** Reads an address from a live image snapshot. */
export function readImage(image: LiveState['image'], address: string, type = ''): Scalar | undefined {
  if (!image) return undefined
  const a = parseAddress(address)
  if (!a) return undefined
  const img = ProcessImage.over(image)
  if (!img.inRange(a)) return undefined
  return img.read(a, type)
}

export function pointMismatch(point: IoPoint, tag: Tag): string | undefined {
  const a = parseAddress(point.address)
  if (!a) return undefined
  const ok = typesForSize(a.size)
  if (!ok.includes(tag.data_type.toUpperCase())) return `${tag.name} is ${tag.data_type}; ${point.terminal} (${point.address}) needs ${ok.join(' / ')}`
  return undefined
}

export function tagsAt(tags: Tag[], address: string): Tag[] {
  const k = address.toUpperCase()
  return tags.filter((t) => t.address?.toUpperCase() === k)
}

export function pointState(point: IoPoint, image: LiveState['image']): { state: DotState; value?: number | boolean } {
  const v = readImage(image, point.address, point.type)
  if (v === undefined) return { state: 'unknown' }
  if (typeof v === 'boolean') return { state: v ? 'power' : 'idle', value: v }
  return { state: v ? 'power' : 'idle', value: v }
}
