// SPDX-License-Identifier: MPL-2.0
#![allow(unused_variables, unused_assignments)]

pub mod compiler;
/// Process-image addressing lives in `plcc-runtime` (it is part of the runtime
/// contract, and front ends that do not link LLVM need it too).
pub use plcc_runtime::direct_address;
pub mod header;

pub use compiler::{Compiler, RuntimeContract, TaskOptions};
