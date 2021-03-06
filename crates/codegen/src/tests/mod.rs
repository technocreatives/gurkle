#[test]
fn schema_with_keywords_works() {
    use crate::{generated_module, schema::Schema, GraphQLClientCodegenOptions};

    let query_string = include_str!("keywords_query.graphql");
    let query = gurkle_parser::parse_query(query_string).expect("Parse keywords query");
    let schema = gurkle_parser::parse_schema(include_str!("keywords_schema.graphql"))
        .expect("Parse keywords schema");
    let schema = Schema::from(schema);

    let options = GraphQLClientCodegenOptions::new();
    let query = crate::query::resolve(&schema, &query).unwrap();

    for (_id, operation) in query.operations() {
        let generated_tokens = generated_module::GeneratedModule {
            schema: &schema,
            operation: &operation.name,
            resolved_query: &query,
            options: &options,
        }
        .to_token_stream()
        .expect("Generate keywords module");
        let generated_code = generated_tokens.to_string();

        // Parse generated code. All keywords should be correctly escaped.
        let r: syn::parse::Result<proc_macro2::TokenStream> = syn::parse2(generated_tokens);
        match r {
            Ok(_) => {
                // Rust keywords should be escaped / renamed now
                assert!(generated_code.contains("pub in_"));
                assert!(generated_code.contains("extern_"));
            }
            Err(e) => {
                panic!("Error: {}\n Generated content: {}\n", e, &generated_code);
            }
        };
    }
}
