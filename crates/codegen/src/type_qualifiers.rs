#[derive(Clone, Debug, PartialEq, Hash)]
pub(crate) enum GraphqlTypeQualifier {
    Required,
    List,
}

impl GraphqlTypeQualifier {
    pub(crate) fn is_required(&self) -> bool {
        *self == GraphqlTypeQualifier::Required
    }
}

pub fn graphql_parser_depth(schema_type: &gurkle_parser::schema::Type) -> usize {
    match schema_type {
        gurkle_parser::schema::Type::ListType(inner) => 1 + graphql_parser_depth(inner),
        gurkle_parser::schema::Type::NonNullType(inner) => 1 + graphql_parser_depth(inner),
        gurkle_parser::schema::Type::NamedType(_) => 0,
    }
}
