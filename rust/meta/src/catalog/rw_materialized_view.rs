use risingwave_common::array::Row;
use risingwave_common::catalog::{Field, Schema};
use risingwave_common::error::Result;
use risingwave_common::types::{DataType, ScalarImpl};
use risingwave_pb::meta::table::Info;
use risingwave_pb::meta::Table;

use crate::model::MetadataModel;
use crate::storage::MetaStore;

pub(crate) const RW_MATERIALIZED_VIEW_NAME: &str = "rw_materialized_view";

lazy_static::lazy_static! {
    pub static ref RW_MATERIALIZED_VIEW_SCHEMA: Schema = Schema {
      fields: vec![
        Field::with_name(DataType::Int32, "id".into()),
        Field::with_name(DataType::Varchar, "rel_name".into()),
        Field::with_name(DataType::Int32, "associated_table_id".into())
      ],
    };
}

pub async fn list_materialized_views<S: MetaStore>(store: &S) -> Result<Vec<Row>> {
    let tables = Table::list(store).await?;
    Ok(tables
        .iter()
        .filter(|table| table.is_materialized_view())
        .map(|table| {
            if let Info::MaterializedView(mv) = table.get_info().unwrap() {
                Row(vec![
                    Some(ScalarImpl::from(table.get_table_ref_id().unwrap().table_id)),
                    Some(ScalarImpl::from(table.get_table_name().to_owned())),
                    mv.associated_table_ref_id
                        .as_ref()
                        .and_then(|table| Some(ScalarImpl::from(table.table_id))),
                ])
            } else {
                unreachable!()
            }
        })
        .collect())
}