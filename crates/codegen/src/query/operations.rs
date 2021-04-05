use super::SelectionId;
use crate::schema::ObjectId;
use heck::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationType {
    Query,
    Mutation,
    Subscription,
}

pub(crate) struct ResolvedOperation {
    pub(crate) name: String,
    pub(crate) query_string: String,
    pub(crate) operation_type: OperationType,
    pub(crate) selection_set: Vec<SelectionId>,
    pub(crate) object_id: ObjectId,
}

impl ResolvedOperation {
    pub(crate) fn to_path_segment(&self) -> String {
        self.name.to_camel_case()
    }
}
