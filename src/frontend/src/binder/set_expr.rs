// Copyright 2022 Singularity Data
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use risingwave_common::catalog::Schema;
use risingwave_common::error::{ErrorCode, Result};
use risingwave_sqlparser::ast::SetExpr;

use crate::binder::{Binder, BoundSelect, BoundValues};
use crate::expr::InputRef;

/// Part of a validated query, without order or limit clause. It may be composed of smaller
/// `BoundSetExpr`s via set operators (e.g. union).
#[derive(Debug)]
pub enum BoundSetExpr {
    Select(Box<BoundSelect>),
    Values(Box<BoundValues>),
}

impl BoundSetExpr {
    /// The schema returned by this [`BoundSetExpr`].

    pub fn schema(&self) -> &Schema {
        match self {
            BoundSetExpr::Select(s) => s.schema(),
            BoundSetExpr::Values(v) => v.schema(),
        }
    }

    pub fn has_correlated_input_ref(&self) -> bool {
        match self {
            BoundSetExpr::Select(s) => s.has_correlated_input_ref(),
            BoundSetExpr::Values(_) => false,
        }
    }

    pub fn get_and_change_correlated_input_ref(&mut self) -> Vec<InputRef> {
        match self {
            BoundSetExpr::Select(s) => s.get_and_change_correlated_input_ref(),
            BoundSetExpr::Values(_) => vec![],
        }
    }
}

impl Binder {
    pub(super) fn bind_set_expr(&mut self, set_expr: SetExpr) -> Result<BoundSetExpr> {
        match set_expr {
            SetExpr::Select(s) => Ok(BoundSetExpr::Select(Box::new(self.bind_select(*s)?))),
            SetExpr::Values(v) => Ok(BoundSetExpr::Values(Box::new(self.bind_values(v, None)?))),
            _ => Err(ErrorCode::NotImplemented(format!("{:?}", set_expr), None.into()).into()),
        }
    }
}
