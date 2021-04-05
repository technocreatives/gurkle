use crate::{
    query::{BoundQuery, OperationId},
    BoxError,
};
use heck::*;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use thiserror::Error;

#[derive(Debug, Error)]
#[error(
    "Could not find an operation named {} in the query document.",
    operation_name
)]
struct OperationNotFound {
    operation_name: String,
}

/// This struct contains the parameters necessary to generate code for a given operation.
pub(crate) struct GeneratedModule<'a> {
    pub operation: &'a str,
    pub query_string: &'a str,
    pub resolved_query: &'a crate::query::Query,
    pub schema: &'a crate::schema::Schema,
    pub options: &'a crate::GraphQLClientCodegenOptions,
}

impl<'a> GeneratedModule<'a> {
    /// Generate the items for the variables and the response that will go inside the module.
    fn build_impls(&self) -> Result<TokenStream, BoxError> {
        Ok(crate::codegen::response_for_query(
            self.root()?,
            &self.options,
            BoundQuery {
                query: self.resolved_query,
                schema: self.schema,
            },
        )?)
    }

    fn root(&self) -> Result<OperationId, OperationNotFound> {
        let op_name = self.options.normalization().operation(self.operation);
        self.resolved_query
            .select_operation(&op_name, *self.options.normalization())
            .map(|op| op.0)
            .ok_or_else(|| OperationNotFound {
                operation_name: op_name.into(),
            })
    }

    /// Generate the module and all the code inside.
    pub(crate) fn to_token_stream(&self) -> Result<TokenStream, BoxError> {
        let module_name = Ident::new(&self.operation.to_snake_case(), Span::call_site());
        let module_visibility = &self.options.module_visibility();
        let operation_name = self.operation;
        let operation_name_ident = self.options.normalization().operation(self.operation);
        let operation_name_ident = Ident::new(&operation_name_ident, Span::call_site());
        let op_request_ident = Ident::new(
            &format!("{}Request", operation_name_ident.to_string()),
            Span::call_site(),
        );

        // Force cargo to refresh the generated code when the query file changes.
        let query_include = self
            .options
            .query_file()
            .map(|path| {
                let path = path.to_str();
                quote!(
                    const __QUERY_WORKAROUND: &str = include_str!(#path);
                )
            })
            .unwrap_or_default();

        let query_string = &self.query_string;
        let impls = self.build_impls()?;

        Ok(quote!(
            #module_visibility mod #module_name {
                #![allow(dead_code)]

                use std::result::Result;

                pub const OPERATION_NAME: &str = #operation_name;
                pub const QUERY: &str = #query_string;

                #query_include

                #impls

                impl Variables {
                    pub async fn execute(self, client: &gurkle::Client) -> Result<#operation_name_ident, gurkle::Error> {
                        use gurkle::Executor;

                        let query_body = gurkle::QueryBody {
                            variables: self,
                            query: QUERY,
                            operation_name: OPERATION_NAME,
                        };

                        client.execute::<#operation_name_ident, _>(query_body).await
                    }
                }

                impl #operation_name_ident {
                    pub fn map<T, F: FnOnce(Self) -> T>(self, f: F) -> T {
                        (f)(self)
                    }
                }
            }

            #module_visibility use #module_name::#operation_name_ident;
            #module_visibility use #module_name::Variables as #op_request_ident;
        ))
    }
}
