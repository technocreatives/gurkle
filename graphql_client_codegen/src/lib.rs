#![deny(missing_docs)]
#![warn(rust_2018_idioms)]
#![allow(clippy::option_option)]

//! Crate for internal use by other graphql-client crates, for code generation.
//!
//! It is not meant to be used directly by users of the library.

use graphql_introspection_query::introspection_response::IntrospectionResponse;
use graphql_parser::schema::parse_schema;
use proc_macro2::TokenStream;
use quote::*;
use schema::Schema;

mod codegen;
mod codegen_options;
/// Deprecation-related code
pub mod deprecation;
/// Contains the [Schema] type and its implementation.
pub mod schema;

mod constants;
mod generated_module;
/// Normalization-related code
pub mod normalization;
mod query;
mod type_qualifiers;

#[cfg(test)]
mod tests;

pub use crate::codegen_options::GraphQLClientCodegenOptions;

use std::{io, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{0}")]
struct GeneralError(String);

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Generates Rust code given a query document, a schema and options.
pub fn generate_module_token_stream(
    query_path: Vec<std::path::PathBuf>,
    schema_path: &Path,
    options: GraphQLClientCodegenOptions,
) -> Result<TokenStream, BoxError> {
    let schema_extension = schema_path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("INVALID");

    // Check the schema cache.
    let schema: Schema = {
        let schema_string = read_file(schema_path)?;
        let schema = match schema_extension {
            "graphql" | "gql" => {
                let s = parse_schema(&schema_string).map_err(|parser_error| GeneralError(format!("Parser error: {}", parser_error)))?;
                Schema::from(s)
            }
            "json" => {
                let parsed: IntrospectionResponse = serde_json::from_str(&schema_string)?;
                Schema::from(parsed)
            }
            extension => return Err(GeneralError(format!("Unsupported extension for the GraphQL schema: {} (only .json and .graphql are supported)", extension)).into())
        };
        schema
    };

    // Load and concatenative all the query files.
    let query_string = query_path.iter()
        .map(|x| read_file(x))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n\n");

    // We need to qualify the query with the path to the crate it is part of
    let (query_string, query) = {
        let query = graphql_parser::parse_query(&query_string)
            .map_err(|err| GeneralError(format!("Query parser error: {}", err)))?;
        (query_string, query)
    };

    let query = crate::query::resolve(&schema, &query)?;

    // Determine which operation we are generating code for. This will be used in operationName.
    let operations = options
        .operation_name
        .as_ref()
        .and_then(|operation_name| query.select_operation(operation_name, *options.normalization()))
        .map(|op| vec![op]);

    let operations = match operations {
        Some(ops) => ops,
        None => query.operations().collect(),
    };

    // The generated modules.
    let mut modules = Vec::with_capacity(operations.len());

    for operation in &operations {
        let generated = generated_module::GeneratedModule {
            query_string: query_string.as_str(),
            schema: &schema,
            resolved_query: &query,
            operation: &operation.1.name,
            options: &options,
        }
        .to_token_stream()?;
        modules.push(generated);
    }

    let modules = quote! { #(#modules)* };

    Ok(modules)
}

#[derive(Debug, Error)]
enum ReadFileError {
    #[error(
        "Could not find file with path: {}\
        Hint: file paths in the GraphQLRequest attribute are relative to the project root (location of the Cargo.toml). Example: query_path = \"src/my_query.graphql\".",
        path
    )]
    FileNotFound {
        path: String,
        #[source]
        io_error: io::Error,
    },
    #[error("Error reading file at: {}", path)]
    ReadError {
        path: String,
        #[source]
        io_error: io::Error,
    },
}

fn read_file(path: &Path) -> Result<String, ReadFileError> {
    use std::fs;
    use std::io::prelude::*;

    let mut out = String::new();
    let mut file = fs::File::open(path).map_err(|io_error| ReadFileError::FileNotFound {
        io_error,
        path: path.display().to_string(),
    })?;

    file.read_to_string(&mut out)
        .map_err(|io_error| ReadFileError::ReadError {
            io_error,
            path: path.display().to_string(),
        })?;
    Ok(out)
}
