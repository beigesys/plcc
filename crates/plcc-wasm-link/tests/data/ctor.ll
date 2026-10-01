; SPDX-License-Identifier: MPL-2.0
; A static constructor: rejected (plcc never emits one).
target triple = "wasm32-unknown-unknown"

@x = global i32 0
@llvm.global_ctors = appending global [1 x { i32, ptr, ptr }] [{ i32, ptr, ptr } { i32 65535, ptr @init, ptr null }]

define internal void @init() {
  store i32 42, ptr @x
  ret void
}

define i32 @get() {
  %v = load i32, ptr @x
  ret i32 %v
}
