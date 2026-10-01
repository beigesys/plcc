; SPDX-License-Identifier: MPL-2.0
; Linker features plcc's fixtures do not all reach: call_indirect (TYPE_INDEX_LEB,
; TABLE_NUMBER_LEB), a function address in code (TABLE_INDEX_SLEB) and in data
; (TABLE_INDEX_I32), an imported function in the table, a data address with an
; addend, hidden / local / weak symbols, and a stack frame.
target triple = "wasm32-unknown-unknown"

declare i32 @ext(i32)
declare void @sink(ptr)

@arr = global [4 x i32] [i32 1, i32 2, i32 3, i32 4]
@mid = global ptr getelementptr (i8, ptr @arr, i32 8)
@zeros = global [64 x i8] zeroinitializer
@hidden_data = hidden global i32 7
@fns = global [3 x ptr] [ptr @double, ptr @ext, ptr @triple]
@str = private unnamed_addr constant [6 x i8] c"hello\00"
@strp = global ptr @str

define internal i32 @triple(i32 %x) {
  %r = mul i32 %x, 3
  ret i32 %r
}

define hidden i32 @double(i32 %x) {
  %r = shl i32 %x, 1
  ret i32 %r
}

define weak i32 @weakfn() {
  ret i32 5
}

define i32 @call_slot(i32 %i, i32 %x) {
  %p = getelementptr [3 x ptr], ptr @fns, i32 0, i32 %i
  %f = load ptr, ptr %p
  %r = call i32 %f(i32 %x)
  ret i32 %r
}

define ptr @addr_of_triple() {
  ret ptr @triple
}

define i32 @call_ptr(ptr %f, i32 %x) {
  %r = call i32 %f(i32 %x)
  ret i32 %r
}

define i32 @read_mid() {
  %p = load ptr, ptr @mid
  %v = load i32, ptr %p
  ret i32 %v
}

define i32 @read_hidden() {
  %v = load i32, ptr @hidden_data
  ret i32 %v
}

define i32 @stack_user(i32 %n) {
  %a = alloca [16 x i32]
  call void @sink(ptr %a)
  %i = and i32 %n, 15
  %p = getelementptr [16 x i32], ptr %a, i32 0, i32 %i
  store i32 %n, ptr %p
  call void @sink(ptr %a)
  %v = load i32, ptr %p
  %z = getelementptr [64 x i8], ptr @zeros, i32 0, i32 %i
  store i8 1, ptr %z
  ret i32 %v
}
