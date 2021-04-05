# Gurkle

[![Github actions Status](https://github.com/bbqsrc/gurkle/workflows/CI/badge.svg?branch=main&event=push)](https://github.com/bbqsrc/gurkle/actions)
[![docs](https://docs.rs/gurkle/badge.svg)](https://docs.rs/gurkle/latest/gurkle/)
[![crates.io](https://img.shields.io/crates/v/gurkle.svg)](https://crates.io/crates/gurkle)

GraphQL client for Rust, with typed requests and responses, and subscriptions!

A fork of [graphql_client](https://github.com/graphql-rust/graphql-client) and consumer of [graphql-ws-rs](https://github.com/technocreatives/graphql-ws-rs).

## Features

- Precise types for query variables and responses.
- Supports GraphQL fragments, objects, unions, inputs, enums, custom scalars and input objects.
- Works in the browser (WebAssembly).
- Subscriptions support (serialization-deserialization only at the moment).
- Copies documentation from the GraphQL schema to the generated Rust code.
- Arbitrary derives on the generated responses.
- Arbitrary custom scalars.
- Supports multiple operations per query document.
- Supports setting GraphQL fields as deprecated and having the Rust compiler check
  their use.

## Usage

- Install the CLI tool.
- Run `gurkle generate --schema-path <your schema> path/to/operations/*.graphql`
- This will generate a `mod.rs` in your current directory.

## Custom scalars

The generated code will reference the scalar types as defined in the server schema. This means you have to provide matching rust types in the scope of the struct under derive. It can be as simple as declarations like `type Email = String;`. This gives you complete freedom on how to treat custom scalars, as long as they can be deserialized.

## Deprecations

The generated code has support for [`@deprecated`](http://facebook.github.io/graphql/June2018/#sec-Field-Deprecation)
field annotations.

## Documentation for the generated modules

You can use `cargo doc --document-private-items` to generate rustdoc documentation on the generated code.

## Examples

See the [examples directory](https://github.com/bbqsrc/gurkle/tree/main/examples) in this repository.

## Code of Conduct

Anyone who interacts with this project in any space, including but not limited to
this GitHub repository, must follow our [Code of Conduct](https://github.com/bbqsrc/gurkle/blob/main/CODE_OF_CONDUCT.md).

## License

Licensed under either of these:

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
  https://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  https://opensource.org/licenses/MIT)

### Contributing

Unless you explicitly state otherwise, any contribution you intentionally submit
for inclusion in the work, as defined in the Apache-2.0 license, shall be
dual-licensed as above, without any additional terms or conditions.
