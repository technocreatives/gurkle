use gurkle::*;
use serde_json::json;

#[derive(GraphQLRequest)]
#[graphql(
    query_path = "tests/fragments/query.graphql",
    schema_path = "tests/fragments/schema.graphql"
)]
pub struct FragmentReference;

#[derive(GraphQLRequest)]
#[graphql(
    query_path = "tests/fragments/query.graphql",
    schema_path = "tests/fragments/schema.graphql"
)]
pub struct SnakeCaseFragment;

#[test]
fn fragment_reference() {
    let valid_response = json!({
        "inFragment": "value",
    });

    let valid_fragment_reference =
        serde_json::from_value::<fragment_reference::ResponseData>(valid_response).unwrap();

    assert_eq!(valid_fragment_reference.in_fragment.unwrap(), "value");
}

#[test]
fn fragments_with_snake_case_name() {
    let valid_response = json!({
        "inFragment": "value",
    });

    let valid_fragment_reference =
        serde_json::from_value::<snake_case_fragment::ResponseData>(valid_response).unwrap();

    assert_eq!(valid_fragment_reference.in_fragment.unwrap(), "value");
}

#[derive(GraphQLRequest)]
#[graphql(
    query_path = "tests/fragments/query.graphql",
    schema_path = "tests/fragments/schema.graphql"
)]
pub struct RecursiveFragmentQuery;

#[test]
fn recursive_fragment() {
    use recursive_fragment_query::*;

    let _ = RecursiveFragment {
        head: Some("ABCD".to_string()),
        tail: Some(Box::new(RecursiveFragment {
            head: Some("EFGH".to_string()),
            tail: None,
        })),
    };
}
