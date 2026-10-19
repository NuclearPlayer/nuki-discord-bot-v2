use indexmap::IndexMap;

use super::operation::{Operation, OperationTable, Param};
use crate::prompts::files::TOOL_RUN_SCRIPT;

/// Makes the TypeScript declarations of the operations, which the LLM reads.
pub fn declarations(table: &OperationTable) -> String {
    namespaces(table)
        .iter()
        .map(|(namespace, operations)| namespace_declaration(namespace, operations))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Makes the JS that defines each namespace object on `globalThis`.
pub fn prelude(table: &OperationTable) -> String {
    namespaces(table)
        .iter()
        .map(|(namespace, operations)| namespace_object(namespace, operations))
        .collect()
}

/// Makes the description of the `run_script` tool.
pub fn tool_description(table: &OperationTable) -> String {
    format!(
        "{TOOL_RUN_SCRIPT}\n```typescript\n{}```",
        declarations(table)
    )
}

/// Groups the operations by namespace. Namespaces and operations keep the sequence of the table.
fn namespaces(table: &OperationTable) -> IndexMap<&'static str, Vec<&Operation>> {
    table
        .operations()
        .iter()
        .fold(IndexMap::new(), |mut namespaces, operation| {
            namespaces
                .entry(operation.namespace)
                .or_default()
                .push(operation);
            namespaces
        })
}

fn namespace_declaration(namespace: &str, operations: &[&Operation]) -> String {
    let functions: String = operations
        .iter()
        .map(|operation| function_declaration(operation))
        .collect();
    format!("declare namespace {namespace} {{\n{functions}}}\n")
}

fn function_declaration(operation: &Operation) -> String {
    let param_docs: String = operation
        .params
        .iter()
        .map(|param| format!("   * @param params.{} {}\n", param.name, param.description))
        .collect();
    format!(
        "  /**\n   * {}\n{param_docs}   */\n  function {}({}): Promise<{}>;\n",
        operation.description,
        operation.name,
        parameter_list(&operation.params),
        operation.returns
    )
}

fn parameter_list(params: &[Param]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let fields = params
        .iter()
        .map(parameter_field)
        .collect::<Vec<_>>()
        .join("; ");
    format!("params: {{ {fields} }}")
}

fn parameter_field(param: &Param) -> String {
    let marker = match param.optional {
        true => "?",
        false => "",
    };
    format!("{}{marker}: {}", param.name, param.ts_type)
}

fn namespace_object(namespace: &str, operations: &[&Operation]) -> String {
    let functions: String = operations
        .iter()
        .map(|operation| function_object(namespace, operation))
        .collect();
    format!("globalThis.{namespace} = {{\n{functions}}};\n")
}

fn function_object(namespace: &str, operation: &Operation) -> String {
    format!(
        "  {name}: async (params) => JSON.parse(await __op(\"{namespace}.{name}\", JSON.stringify(params ?? {{}}))),\n",
        name = operation.name
    )
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn operation(
        namespace: &'static str,
        name: &'static str,
        description: &'static str,
        params: Vec<Param>,
        returns: &'static str,
    ) -> Operation {
        Operation::typed(
            namespace,
            name,
            description,
            params,
            returns,
            |_: Value| async { Ok::<_, String>(Value::Null) },
        )
    }

    #[test]
    fn declarations_match_the_format_for_a_table_with_two_namespaces() {
        let table = OperationTable::new(vec![
            operation(
                "discord",
                "getMessage",
                "Gets one message.",
                vec![
                    Param {
                        name: "channelId",
                        ts_type: "string",
                        optional: false,
                        description: "The ID of the channel.",
                    },
                    Param {
                        name: "limit",
                        ts_type: "number",
                        optional: true,
                        description: "The maximum number of messages.",
                    },
                ],
                "Message",
            ),
            operation(
                "repo",
                "readFile",
                "Reads a file.",
                vec![Param {
                    name: "path",
                    ts_type: "string",
                    optional: false,
                    description: "The path of the file.",
                }],
                "string",
            ),
            operation(
                "discord",
                "react",
                "Adds a reaction.",
                vec![Param {
                    name: "emoji",
                    ts_type: "string",
                    optional: false,
                    description: "The emoji.",
                }],
                "void",
            ),
        ]);

        assert_eq!(
            declarations(&table),
            "declare namespace discord {
  /**
   * Gets one message.
   * @param params.channelId The ID of the channel.
   * @param params.limit The maximum number of messages.
   */
  function getMessage(params: { channelId: string; limit?: number }): Promise<Message>;
  /**
   * Adds a reaction.
   * @param params.emoji The emoji.
   */
  function react(params: { emoji: string }): Promise<void>;
}

declare namespace repo {
  /**
   * Reads a file.
   * @param params.path The path of the file.
   */
  function readFile(params: { path: string }): Promise<string>;
}
"
        );
    }

    #[test]
    fn declarations_of_a_function_without_params() {
        let table = OperationTable::new(vec![operation(
            "repo",
            "list",
            "Lists the repositories.",
            vec![],
            "string[]",
        )]);

        assert_eq!(
            declarations(&table),
            "declare namespace repo {
  /**
   * Lists the repositories.
   */
  function list(): Promise<string[]>;
}
"
        );
    }

    #[test]
    fn tool_description_has_the_instructions_and_the_declarations() {
        let table = OperationTable::new(vec![operation(
            "repo",
            "list",
            "Lists the repositories.",
            vec![],
            "string[]",
        )]);

        assert_eq!(
            tool_description(&table),
            "Runs JavaScript in a sandbox. The code is the body of an async function, so you can use `await`, and `return` gives the result. The output of `console.log` is returned too. The API below is the only API. It is not discord.js: there is no `client` object and no `fetch`.

```typescript
declare namespace repo {
  /**
   * Lists the repositories.
   */
  function list(): Promise<string[]>;
}
```"
        );
    }
}
