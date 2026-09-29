// SPDX-License-Identifier: MPL-2.0

//! I/O mapping (stub).

use crate::emit::Out;
use crate::scope::Ctx;
use crate::types::{Ty, TypeEnv};

#[derive(Default, Clone, Debug)]
pub struct IoMap {}

impl IoMap {
    pub fn parse(_text: &str) -> Result<IoMap, String> {
        Err("--io-map is not implemented yet".into())
    }

    pub(crate) fn at_for(&self, _logix: &str, _ty: &Ty, _env: &TypeEnv) -> Option<String> {
        None
    }

    pub(crate) fn copy_in(&self, _ctx: &Ctx) -> Out {
        Out::new()
    }

    pub(crate) fn copy_out(&self, _ctx: &Ctx) -> Out {
        Out::new()
    }
}
