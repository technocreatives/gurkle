use gurkle::*;

#[allow(dead_code)]
#[derive(GraphQLRequest)]
#[graphql(
    schema_path = "tests/fragment_chain/schema.graphql",
    query_path = "tests/fragment_chain/query.graphql"
)]
struct Q;
