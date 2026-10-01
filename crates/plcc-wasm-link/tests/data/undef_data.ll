; SPDX-License-Identifier: MPL-2.0
; A reference to an undefined data symbol: rejected.
target triple = "wasm32-unknown-unknown"

@elsewhere = external global i32

define i32 @get() {
  %v = load i32, ptr @elsewhere
  ret i32 %v
}
