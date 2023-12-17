use crate::{
    error::{Error, Result},
    stream::BlockingRecv,
};
use serde_json::Value;
use smallvec::SmallVec;
use starlark::{
    environment::{FrozenModule, Globals, GlobalsBuilder, LibraryExtension, Module},
    eval::Evaluator,
    syntax::{AstModule, Dialect, DialectTypes},
    values::{function::FUNCTION_TYPE, Heap, Value as AllocValue},
};

pub struct Script {
    module: FrozenModule,
}

pub enum InputValue<'a> {
    Json(Value),
    StrRef(&'a str),
    Stream(Box<dyn BlockingRecv>),
}

impl<'a> InputValue<'a> {
    fn alloc(self, heap: &Heap) -> AllocValue<'_> {
        match self {
            InputValue::Json(v) => heap.alloc(v),
            InputValue::StrRef(s) => heap.alloc(s),
            InputValue::Stream(stream) => heap.alloc(crate::stream::Stream { stream }),
        }
    }
}

impl Script {
    pub fn new(content: impl Into<String>) -> Result<Self> {
        let ast = AstModule::parse(
            "__main__",
            content.into(),
            &Dialect {
                enable_def: true,
                enable_lambda: true,
                enable_load: true,
                enable_keyword_only_arguments: true,
                enable_types: DialectTypes::Enable,
                enable_load_reexport: true,
                enable_top_level_stmt: true,
                enable_f_strings: true,
                ..Default::default()
            },
        )
        .map_err(Error::Starlark)?;

        let globals: Globals = GlobalsBuilder::extended_by(&[
            LibraryExtension::Map,
            LibraryExtension::Filter,
            LibraryExtension::Print,
            LibraryExtension::Json,
        ])
        .build();

        let module = Module::new();
        {
            let mut eval = Evaluator::new(&module);
            eval.enable_static_typechecking(true);
            eval.eval_module(ast, &globals).map_err(Error::Starlark)?;
        }
        Ok(Self {
            module: module.freeze().map_err(Error::Starlark)?,
        })
    }

    pub fn run<'a>(
        &self,
        name: &str,
        positional: impl IntoIterator<Item = InputValue<'a>>,
        named: impl IntoIterator<Item = (&'a str, InputValue<'a>)>,
    ) -> Result<Value> {
        let func = self.module.get(name).map_err(Error::Starlark)?;
        let v = func.value();
        if v.get_type() != FUNCTION_TYPE {
            return v.to_json_value().map_err(Error::Starlark);
        }

        let module = Module::new();
        let heap = module.heap();
        let positional = positional
            .into_iter()
            .map(|p| p.alloc(heap))
            .collect::<SmallVec<[_; 4]>>();
        let named = named
            .into_iter()
            .map(|(s, v)| (s, v.alloc(heap)))
            .collect::<SmallVec<[_; 2]>>();
        let result = Evaluator::new(&module)
            .eval_function(v, &positional, &named)
            .map_err(Error::Starlark)?;
        result.to_json_value().map_err(Error::Starlark)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::sync::mpsc::channel;

    use super::*;

    #[tokio::test]
    async fn test() {
        let content = r#"
i = 1
def hello(n):
    n += i
    return f"hello {n}"

def sum(i):
    total = 0
    for x in i:
        total += x
    return total
i += 21
"#;
        let s = Script::new(content).unwrap();
        let x = s.run("hello", [InputValue::Json(json!(32))], []).unwrap();
        assert_eq!(x, json!("hello 54"));

        let x = s.run("i", [], []).unwrap();
        assert_eq!(x, json!(22));

        let (tx, rx) = channel::<Value>(12);
        tx.send(json!(1)).await.unwrap();
        tx.send(json!(2)).await.unwrap();
        drop(tx);

        let tot =
            tokio::task::spawn_blocking(move || s.run("sum", [InputValue::Stream(rx.into())], []))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(tot, json!(3));
    }
}
