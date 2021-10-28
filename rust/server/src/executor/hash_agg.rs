use std::collections::HashMap;
use std::sync::Arc;
use std::{mem, vec};

use crate::error::ErrorCode::ProtobufError;
use itertools::Itertools;

use protobuf::Message;
use risingwave_proto::plan::{HashAggNode, PlanNode_PlanNodeType};

use crate::array::column::Column;
use crate::array::{DataChunk, RwError};
use crate::error::{ErrorCode, Result};
use crate::executor::hash_map::{HashKey, PrecomputedBuildHasher, SerializedKey};
use crate::executor::ExecutorResult::Batch;
use crate::executor::{BoxedExecutor, Field, Schema};
use crate::types::{DataType, DataTypeRef};
use crate::vector_op::agg::AggStateFactory;
use crate::vector_op::agg::BoxedAggState;

use super::{BoxedExecutorBuilder, Executor, ExecutorBuilder, ExecutorResult};

type AggHashMap<K> = HashMap<K, Vec<BoxedAggState>, PrecomputedBuildHasher>;
pub(super) struct HashAggExecutorBuilder;
impl BoxedExecutorBuilder for HashAggExecutorBuilder {
    fn new_boxed_executor(source: &ExecutorBuilder) -> Result<BoxedExecutor> {
        ensure!(source.plan_node().get_node_type() == PlanNode_PlanNodeType::HASH_AGG);
        ensure!(source.plan_node().get_children().len() == 1);
        let proto_child = source
            .plan_node()
            .get_children()
            .get(0)
            .ok_or_else(|| ErrorCode::InternalError(String::from("")))?;
        let child = ExecutorBuilder::new(proto_child, source.global_task_env().clone()).build()?;

        let hash_agg_node =
            HashAggNode::parse_from_bytes(source.plan_node().get_body().get_value())
                .map_err(|e| RwError::from(ProtobufError(e)))?;

        let group_key_columns = hash_agg_node
            .get_group_keys()
            .iter()
            .map(|x| *x as usize)
            .collect_vec();

        let agg_factories = hash_agg_node
            .get_agg_calls()
            .iter()
            .map(AggStateFactory::new)
            .collect::<Result<Vec<AggStateFactory>>>()?;

        let child_schema = child.schema();

        let group_key_types = group_key_columns
            .iter()
            .map(|i| child_schema.fields[*i].data_type.clone())
            .collect_vec();

        let fields = group_key_types
            .iter()
            .cloned()
            .chain(agg_factories.iter().map(|e| e.get_return_type()))
            .map(|t| Field { data_type: t })
            .collect::<Vec<Field>>();

        // todo, calc the K type
        Ok(Box::new(HashAggExecutor::<SerializedKey> {
            agg_factories,
            group_key_columns,
            child,
            groups: AggHashMap::<SerializedKey>::default(),
            group_key_types,
            done: false,
            schema: Schema { fields },
        }) as BoxedExecutor)
    }
}
/// `HashAggExecutor` implements the hash aggregate algorithm.
pub(super) struct HashAggExecutor<K> {
    /// factories to construct aggrator for each groups
    agg_factories: Vec<AggStateFactory>,
    /// Column indexes of keys that specify a group
    group_key_columns: Vec<usize>,
    /// child executor
    child: BoxedExecutor,
    /// Hash map for each agg groups
    groups: AggHashMap<K>,
    /// if all results have been outputed
    done: bool,
    /// the data types of key columns
    group_key_types: Vec<DataTypeRef>,
    schema: Schema,
}

#[async_trait::async_trait]
impl<K: HashKey + Send + Sync> Executor for HashAggExecutor<K> {
    fn init(&mut self) -> Result<()> {
        self.child.init()
    }

    async fn execute(&mut self) -> Result<ExecutorResult> {
        if self.done {
            return Ok(ExecutorResult::Done);
        }
        while let Batch(chunk) = self.child.execute().await? {
            let keys = K::build(self.group_key_columns.as_slice(), &chunk)?;
            for (row_id, key) in keys.into_iter().enumerate() {
                let mut err_flag = None;
                let states: &mut Vec<BoxedAggState> = self.groups.entry(key).or_insert_with(|| {
                    self.agg_factories
                        .iter()
                        .map(|state_factory| state_factory.create_agg_state())
                        .collect::<Result<Vec<_>>>()
                        .unwrap_or_else(|x| {
                            err_flag = Some(x);
                            vec![]
                        })
                });
                if let Some(err) = err_flag {
                    return Err(err);
                }
                // TODO: currently not a vectorized implementation
                states
                    .iter_mut()
                    .for_each(|state| state.update_with_row(&chunk, row_id).unwrap());
            }
        }
        let cardinality = self.groups.len();

        let mut group_builders = self
            .group_key_types
            .iter()
            .map(|datatype| DataType::create_array_builder(datatype.clone(), cardinality))
            .collect::<Result<Vec<_>>>()?;

        let mut agg_builders = self
            .agg_factories
            .iter()
            .map(|agg_factory| {
                DataType::create_array_builder(agg_factory.get_return_type(), cardinality)
            })
            .collect::<Result<Vec<_>>>()?;

        for (key, states) in mem::take(&mut self.groups).into_iter() {
            key.deserialize_to_builders(&mut group_builders)?;
            states
                .into_iter()
                .zip(&mut agg_builders)
                .try_for_each(|(aggregator, builder)| aggregator.output(builder))?;
        }

        let columns = mem::take(&mut self.group_key_types)
            .into_iter()
            .chain(
                self.agg_factories
                    .iter()
                    .map(|agg_factory| agg_factory.get_return_type()),
            )
            .zip(group_builders.into_iter().chain(agg_builders))
            .map(|(t, b)| Ok(Column::new(Arc::new(b.finish()?), t)))
            .collect::<Result<Vec<_>>>()?;

        let ret = DataChunk::builder().columns(columns).build();
        self.done = true;
        Ok(ExecutorResult::Batch(ret))
    }

    fn clean(&mut self) -> Result<()> {
        self.child.clean()
    }

    fn schema(&self) -> &Schema {
        &self.schema
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{I32Array, I64Array};
    use crate::array_nonnull;
    use crate::executor::test_utils::{diff_executor_output, MockExecutor};
    use crate::executor::{Field, Schema};
    use crate::types::{Int32Type, Int64Type};

    use pb_construct::make_proto;

    use risingwave_proto::data::{DataType as DataTypeProto, DataType_TypeName};
    use risingwave_proto::expr::{AggCall, AggCall_Arg, AggCall_Type, InputRefExpr};

    #[tokio::test]
    async fn execute_int32_grouped() {
        let key1_col = Arc::new(array_nonnull! { I32Array, [0,1,0,1,1,0,1,0] }.into());
        let key2_col = Arc::new(array_nonnull! { I32Array, [1,1,0,1,0,0,1,1] }.into());
        let sum_col = Arc::new(array_nonnull! { I32Array,  [1,1,1,2,1,2,3,2] }.into());

        let t32 = Int32Type::create(false);
        let t64 = Int64Type::create(false);

        let src_exec = MockExecutor::with_chunk(
            DataChunk::builder()
                .columns(vec![
                    Column::new(key1_col, t32.clone()),
                    Column::new(key2_col, t32.clone()),
                    Column::new(sum_col, t32.clone()),
                ])
                .build(),
            Schema {
                fields: vec![
                    Field {
                        data_type: t32.clone(),
                    },
                    Field {
                        data_type: t32.clone(),
                    },
                    Field {
                        data_type: t32.clone(),
                    },
                ],
            },
        );

        let proto = make_proto!(AggCall, {
          field_type: AggCall_Type::SUM,
          return_type: make_proto!(DataTypeProto, {
            type_name: DataType_TypeName::INT64
          }),
          args: vec![make_proto!(AggCall_Arg, {
            input: make_proto!(InputRefExpr, {column_idx: 2}),
            field_type: make_proto!(DataTypeProto, {
              type_name: DataType_TypeName::INT32
            })
          })].into()
        });

        let agg_factory = AggStateFactory::new(&proto).unwrap();
        let schema = Schema {
            fields: vec![
                Field {
                    data_type: t32.clone(),
                },
                Field {
                    data_type: t32.clone(),
                },
                Field {
                    data_type: t64.clone(),
                },
            ],
        };
        let actual_exec = HashAggExecutor {
            agg_factories: vec![agg_factory],
            group_key_columns: vec![0, 1],
            child: Box::new(src_exec),
            groups: AggHashMap::<SerializedKey>::default(),
            group_key_types: vec![t32.clone(), t32.clone()],
            done: false,
            schema: schema.clone(),
        };
        // TODO: currently the order is fixed
        let group1_col = Arc::new(array_nonnull! { I32Array, [0,1,0,1] }.into());
        let group2_col = Arc::new(array_nonnull! { I32Array, [0,1,1,0] }.into());
        let anssum_col = Arc::new(array_nonnull! { I64Array, [3,6,3,1] }.into());

        let expect_exec = MockExecutor::with_chunk(
            DataChunk::builder()
                .columns(vec![
                    Column::new(group1_col, t32.clone()),
                    Column::new(group2_col, t32.clone()),
                    Column::new(anssum_col, t64.clone()),
                ])
                .build(),
            schema,
        );
        diff_executor_output(Box::new(actual_exec), Box::new(expect_exec)).await;
    }
}
