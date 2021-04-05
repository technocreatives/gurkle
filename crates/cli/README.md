# GraphQL client CLI

This is still a WIP, the main use for it now is to download the `schema.json` from a GraphQL endpoint, which you can also do with [the Apollo CLI](https://github.com/apollographql/apollo-tooling#apollo-clientdownload-schema-output).

## Install

```bash
cargo install graphql_client_cli --force
```

## introspect schema

```
Get the schema from a live GraphQL API. The schema is printed to stdout.

USAGE:
    graphql-client introspect-schema [FLAGS] [OPTIONS] <schema_location>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
        --no-ssl     Set this option to disable ssl certificate verification. Default value is false.
                     ssl verification is turned on by default.

OPTIONS:
        --authorization <authorization>    Set the contents of the Authorizaiton header.
        --header <headers>...              Specify custom headers. --header 'X-Name: Value'
        --output <output>                  Where to write the JSON for the introspected schema.

ARGS:
    <schema_location>    The URL of a GraphQL endpoint to introspect.
```

## generate client code

```
USAGE:
    graphql-client generate [FLAGS] [OPTIONS] <query_path> --schema-path <schema_path>

FLAGS:
    -h, --help             Prints help information
        --no-formatting    If you don't want to execute rustfmt to generated code, set this option. Default value is
                           false. Formating feature is disabled as default installation.
    -V, --version          Prints version information

OPTIONS:
    -a, --additional-derives <additional_derives>
            Additional derives that will be added to the generated structs and enums for the response and the variables.
            --additional-derives='Serialize,PartialEq'
    -d, --deprecation-strategy <deprecation_strategy>
            You can choose deprecation strategy from allow, deny, or warn. Default value is warn.

    -m, --module-visibility <module_visibility>
            You can choose module and target struct visibility from pub and private. Default value is pub.

    -o, --output-directory <output_directory>            The directory in which the code will be generated
    -s, --schema-path <schema_path>                      Path to GraphQL schema file (.json or .graphql).
    -o, --selected-operation <selected_operation>
            Name of target query. If you don't set this parameter, cli generate all queries in query file.


ARGS:
    <query_path>    Path to the GraphQL query file.
```

If you want to use formatting feature, you should install like this.

```bash
cargo install graphql_client_cli --features rustfmt --force
```