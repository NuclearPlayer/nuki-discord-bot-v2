use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;

use futures::FutureExt;
use futures::future::{self, BoxFuture};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// One parameter of an operation, as the TypeScript declarations show it.
pub struct Param {
    pub name: &'static str,
    pub ts_type: &'static str,
    pub optional: bool,
    pub description: &'static str,
}

pub type OperationHandler =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, String>> + Send + Sync>;

/// A Rust function that scripts can call as `{namespace}.{name}(params)`.
pub struct Operation {
    pub namespace: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub params: Vec<Param>,
    /// A TypeScript type expression.
    pub returns: &'static str,
    pub handler: OperationHandler,
}

impl Operation {
    /// Makes an operation whose handler parses the JSON parameters into `P`,
    /// calls `f`, and serializes its result.
    pub fn typed<P, R, E, F, Fut>(
        namespace: &'static str,
        name: &'static str,
        description: &'static str,
        params: Vec<Param>,
        returns: &'static str,
        f: F,
    ) -> Self
    where
        P: DeserializeOwned,
        R: Serialize + 'static,
        E: Display + 'static,
        F: Fn(P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, E>> + Send + 'static,
    {
        let handler: OperationHandler =
            Arc::new(move |json| match serde_json::from_value::<P>(json) {
                Ok(parsed) => call(f(parsed)).boxed(),
                Err(error) => future::ready(Err(format!("invalid parameters: {error}"))).boxed(),
            });
        Self {
            namespace,
            name,
            description,
            params,
            returns,
            handler,
        }
    }
}

async fn call<R: Serialize, E: Display>(
    operation: impl Future<Output = Result<R, E>>,
) -> Result<Value, String> {
    let result = operation.await.map_err(|error| error.to_string())?;
    serde_json::to_value(result).map_err(|error| error.to_string())
}

/// The operations that scripts can call, in the sequence the declarations show them.
pub struct OperationTable {
    operations: Vec<Operation>,
}

impl OperationTable {
    pub fn new(operations: Vec<Operation>) -> Self {
        Self { operations }
    }

    pub fn merge(tables: impl IntoIterator<Item = OperationTable>) -> Self {
        Self::new(
            tables
                .into_iter()
                .flat_map(|table| table.operations)
                .collect(),
        )
    }

    /// Finds an operation by its full name, `namespace.name`.
    pub fn find(&self, full_name: &str) -> Option<&Operation> {
        let (namespace, name) = full_name.split_once('.')?;
        self.operations
            .iter()
            .find(|operation| operation.namespace == namespace && operation.name == name)
    }

    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Deserialize)]
    struct Message {
        text: String,
    }

    #[tokio::test]
    async fn typed_operation_rejects_invalid_params_with_the_serde_message() {
        let operation = Operation::typed(
            "test",
            "say",
            "Says a text.",
            vec![],
            "string",
            |message: Message| async { Ok::<_, String>(message.text) },
        );

        let result = (operation.handler)(json!({ "count": 1 })).await;

        assert_eq!(
            result,
            Err("invalid parameters: missing field `text`".to_owned())
        );
    }
}
