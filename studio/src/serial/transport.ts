// SPDX-License-Identifier: MPL-2.0

/** A line-oriented serial link. WebSerial in the browser, a fake in tests. */
export interface SerialTransport {
  open(): Promise<void>
  close(): Promise<void>
  /** Sends `text` followed by a newline. */
  writeLine(text: string): Promise<void>
  /** Called for every received line (without the line terminator). Returns an unsubscribe. */
  onLine(cb: (line: string) => void): () => void
  /** Called when the link closes, by us or by the device going away. Returns an unsubscribe. */
  onClose(cb: (reason?: string) => void): () => void
  readonly isOpen: boolean
}

/** Shared listener bookkeeping for transports. */
export class Listeners<T extends unknown[]> {
  private set = new Set<(...args: T) => void>()
  add(cb: (...args: T) => void): () => void {
    this.set.add(cb)
    return () => this.set.delete(cb)
  }
  emit(...args: T): void {
    for (const cb of [...this.set]) cb(...args)
  }
}
