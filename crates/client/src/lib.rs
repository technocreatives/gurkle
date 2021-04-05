//! The top-level documentation resides on the [project README](https://github.com/graphql-rust/graphql-client) at the moment.
//!
//! The main interface to this library is the custom derive that generates modules from a GraphQL query and schema. See the docs for the [`GraphQLRequest`] trait for a full example.

#![warn(missing_docs)]
#![deny(rust_2018_idioms)]

use reqwest::Url;
use serde::*;

#[cfg(feature = "web")]
pub mod web;

use std::fmt::{self, Display};
use std::pin::Pin;
use std::{collections::HashMap, future::Future};

pub trait Executor {
    /// Execute
    fn execute<'a, T, V>(
        &'a self,
        request_body: QueryBody<V>,
    ) -> Pin<Box<dyn Future<Output = Result<T, Error>> + 'a>>
    where
        V: Serialize + 'a,
        T: for<'de> Deserialize<'de> + 'a;
}

pub struct Client {
    http_endpoint: Url,
    http: reqwest::Client,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The response contains errors.")]
    GraphQL(Vec<GraphQLError>),

    #[error("An HTTP error occurred.")]
    Http(#[from] reqwest::Error),

    #[error("An error parsing JSON response occurred.")]
    Json(#[from] serde_json::Error),

    #[error("The response body is empty.")]
    Empty,
}

impl Client {
    /// Create a new Gurkle client.
    pub fn new(endpoint: &Url) -> Self {
        Self {
            http_endpoint: endpoint.clone(),
            http: reqwest::Client::new(),
        }
    }

    async fn execute_inner<T, V>(&self, request_body: QueryBody<V>) -> Result<T, Error>
    where
        V: Serialize,
        T: for<'de> Deserialize<'de>,
    {
        let response = self
            .http
            .post(self.http_endpoint.clone())
            .json(&request_body)
            .send()
            .await?;
        let body: Response<T> = response.json().await?;

        match (body.data, body.errors) {
            (None, None) => Err(Error::Empty),
            (None, Some(errs)) => Err(Error::GraphQL(errs)),
            (Some(data), _) => Ok(data),
        }
    }
}

impl Executor for Client {
    fn execute<'a, T, V>(
        &'a self,
        request_body: QueryBody<V>,
    ) -> Pin<Box<dyn Future<Output = Result<T, Error>> + 'a>>
    where
        V: Serialize + 'a,
        T: for<'de> Deserialize<'de> + 'a,
    {
        Box::pin(self.execute_inner(request_body))
    }
}

/// The form in which queries are sent over HTTP in most implementations. This will be built using the [`GraphQLRequest`] trait normally.
#[derive(Debug, Serialize, Deserialize)]
pub struct QueryBody<Variables> {
    /// The values for the variables. They must match those declared in the queries. This should be the `Variables` struct from the generated module corresponding to the query.
    pub variables: Variables,
    /// The GraphQL query, as a string.
    pub query: &'static str,
    /// The GraphQL operation name, as a string.
    #[serde(rename = "operationName")]
    pub operation_name: &'static str,
}

/// Represents a location inside a query string. Used in errors. See [`GraphQLError`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Location {
    /// The line number in the query string where the error originated (starting from 1).
    pub line: i32,
    /// The column number in the query string where the error originated (starting from 1).
    pub column: i32,
}

/// Part of a path in a query. It can be an object key or an array index. See [`GraphQLError`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PathFragment {
    /// A key inside an object
    Key(String),
    /// An index inside an array
    Index(i32),
}

impl Display for PathFragment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            PathFragment::Key(ref key) => write!(f, "{}", key),
            PathFragment::Index(ref idx) => write!(f, "{}", idx),
        }
    }
}

/// An element in the top-level `errors` array of a response body.
///
/// This tries to be as close to the spec as possible.
///
/// [Spec](https://github.com/facebook/graphql/blob/master/spec/Section%207%20--%20Response.md)
///
///
/// ```
/// # use serde_json::json;
/// # use serde::Deserialize;
/// # use gurkle::GraphQLRequest;
/// # use std::error::GraphQLError;
/// #
/// # #[derive(Debug, Deserialize, PartialEq)]
/// # struct ResponseData {
/// #     something: i32
/// # }
/// #
/// # fn main() -> Result<(), Box<dyn GraphQLError>> {
/// use gurkle::*;
///
/// let body: Response<ResponseData> = serde_json::from_value(json!({
///     "data": null,
///     "errors": [
///         {
///             "message": "The server crashed. Sorry.",
///             "locations": [{ "line": 1, "column": 1 }]
///         },
///         {
///             "message": "Seismic activity detected",
///             "path": ["underground", 20]
///         },
///      ],
/// }))?;
///
/// let expected: Response<ResponseData> = Response {
///     data: None,
///     errors: Some(vec![
///         GraphQLError {
///             message: "The server crashed. Sorry.".to_owned(),
///             locations: Some(vec![
///                 Location {
///                     line: 1,
///                     column: 1,
///                 }
///             ]),
///             path: None,
///             extensions: None,
///         },
///         GraphQLError {
///             message: "Seismic activity detected".to_owned(),
///             locations: None,
///             path: Some(vec![
///                 PathFragment::Key("underground".into()),
///                 PathFragment::Index(20),
///             ]),
///             extensions: None,
///         },
///     ]),
/// };
///
/// assert_eq!(body, expected);
///
/// #     Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphQLError {
    /// The human-readable error message. This is the only required field.
    pub message: String,
    /// Which locations in the query the error applies to.
    pub locations: Option<Vec<Location>>,
    /// Which path in the query the error applies to, e.g. `["users", 0, "email"]`.
    pub path: Option<Vec<PathFragment>>,
    /// Additional errors. Their exact format is defined by the server.
    pub extensions: Option<HashMap<String, serde_json::Value>>,
}

impl Display for GraphQLError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use `/` as a separator like JSON Pointer.
        let path = self
            .path
            .as_ref()
            .map(|fragments| {
                fragments
                    .iter()
                    .fold(String::new(), |mut acc, item| {
                        acc.push_str(&format!("{}/", item));
                        acc
                    })
                    .trim_end_matches('/')
                    .to_string()
            })
            .unwrap_or_else(|| "<query>".to_string());

        // Get the location of the error. We'll use just the first location for this.
        let loc = self
            .locations
            .as_ref()
            .and_then(|locations| locations.iter().next())
            .cloned()
            .unwrap_or_else(Location::default);

        write!(f, "{}:{}:{}: {}", path, loc.line, loc.column, self.message)
    }
}

/// The generic shape taken by the responses of GraphQL APIs.
///
/// This will generally be used with the `ResponseData` struct from a derived module.
///
/// [Spec](https://github.com/facebook/graphql/blob/master/spec/Section%207%20--%20Response.md)
///
/// ```
/// # use serde_json::json;
/// # use serde::Deserialize;
/// # use gurkle::GraphQLRequest;
/// # use std::error::GraphQLError;
/// #
/// # #[derive(Debug, Deserialize, PartialEq)]
/// # struct User {
/// #     id: i32,
/// # }
/// #
/// # #[derive(Debug, Deserialize, PartialEq)]
/// # struct Dog {
/// #     name: String
/// # }
/// #
/// # #[derive(Debug, Deserialize, PartialEq)]
/// # struct ResponseData {
/// #     users: Vec<User>,
/// #     dogs: Vec<Dog>,
/// # }
/// #
/// # fn main() -> Result<(), Box<dyn GraphQLError>> {
/// use gurkle::Response;
///
/// let body: Response<ResponseData> = serde_json::from_value(json!({
///     "data": {
///         "users": [{"id": 13}],
///         "dogs": [{"name": "Strelka"}],
///     },
///     "errors": [],
/// }))?;
///
/// let expected: Response<ResponseData> = Response {
///     data: Some(ResponseData {
///         users: vec![User { id: 13 }],
///         dogs: vec![Dog { name: "Strelka".to_owned() }],
///     }),
///     errors: Some(vec![]),
/// };
///
/// assert_eq!(body, expected);
///
/// #     Ok(())
/// # }
/// ```
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Response<Data> {
    /// The absent, partial or complete response data.
    pub data: Option<Data>,
    /// The top-level errors returned by the server.
    pub errors: Option<Vec<GraphQLError>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn graphql_error_works_with_just_message() {
        let err = json!({
            "message": "I accidentally your whole query"
        });

        let deserialized_error: GraphQLError = serde_json::from_value(err).unwrap();

        assert_eq!(
            deserialized_error,
            GraphQLError {
                message: "I accidentally your whole query".to_string(),
                locations: None,
                path: None,
                extensions: None,
            }
        )
    }

    #[test]
    fn full_graphql_error_deserialization() {
        let err = json!({
            "message": "I accidentally your whole query",
            "locations": [{ "line": 3, "column": 13}, {"line": 56, "column": 1}],
            "path": ["home", "alone", 3, "rating"]
        });

        let deserialized_error: GraphQLError = serde_json::from_value(err).unwrap();

        assert_eq!(
            deserialized_error,
            GraphQLError {
                message: "I accidentally your whole query".to_string(),
                locations: Some(vec![
                    Location {
                        line: 3,
                        column: 13,
                    },
                    Location {
                        line: 56,
                        column: 1,
                    },
                ]),
                path: Some(vec![
                    PathFragment::Key("home".to_owned()),
                    PathFragment::Key("alone".to_owned()),
                    PathFragment::Index(3),
                    PathFragment::Key("rating".to_owned()),
                ]),
                extensions: None,
            }
        )
    }

    #[test]
    fn full_graphql_error_with_extensions_deserialization() {
        let err = json!({
            "message": "I accidentally your whole query",
            "locations": [{ "line": 3, "column": 13}, {"line": 56, "column": 1}],
            "path": ["home", "alone", 3, "rating"],
            "extensions": {
                "code": "CAN_NOT_FETCH_BY_ID",
                "timestamp": "Fri Feb 9 14:33:09 UTC 2018"
            }
        });

        let deserialized_error: GraphQLError = serde_json::from_value(err).unwrap();

        let mut expected_extensions = HashMap::new();
        expected_extensions.insert("code".to_owned(), json!("CAN_NOT_FETCH_BY_ID"));
        expected_extensions.insert("timestamp".to_owned(), json!("Fri Feb 9 14:33:09 UTC 2018"));
        let expected_extensions = Some(expected_extensions);

        assert_eq!(
            deserialized_error,
            GraphQLError {
                message: "I accidentally your whole query".to_string(),
                locations: Some(vec![
                    Location {
                        line: 3,
                        column: 13,
                    },
                    Location {
                        line: 56,
                        column: 1,
                    },
                ]),
                path: Some(vec![
                    PathFragment::Key("home".to_owned()),
                    PathFragment::Key("alone".to_owned()),
                    PathFragment::Index(3),
                    PathFragment::Key("rating".to_owned()),
                ]),
                extensions: expected_extensions,
            }
        )
    }
}
