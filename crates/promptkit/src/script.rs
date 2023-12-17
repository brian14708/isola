use crate::error::{Error, Result};
use starlark::{
    environment::{FrozenModule, Globals, GlobalsBuilder, LibraryExtension, Module},
    eval::Evaluator,
    syntax::{AstModule, Dialect, DialectTypes},
};

pub struct Script {
    module: FrozenModule,
}

impl Script {
    pub fn new(content: impl Into<String>) -> Result<Self> {
        let ast = AstModule::parse(
            "__main__",
            content.into(),
            &Dialect {
                enable_def: true,
                enable_lambda: true,
                enable_load: false,
                enable_keyword_only_arguments: true,
                enable_types: DialectTypes::Enable,
                enable_load_reexport: false,
                enable_top_level_stmt: false,
                enable_f_strings: true,
                ..Default::default()
            },
        )
        .map_err(Error::Starlark)?;
        let globals: Globals = GlobalsBuilder::extended_by(&[
            LibraryExtension::Map,
            LibraryExtension::Filter,
            LibraryExtension::Json,
        ])
        .build();
        let module: Module = Module::new();
        let mut eval: Evaluator = Evaluator::new(&module);
        eval.enable_static_typechecking(true);
        eval.eval_module(ast, &globals).map_err(Error::Starlark)?;
        drop(eval);
        let frozen = module.freeze().map_err(Error::Starlark)?;
        Ok(Self { module: frozen })
    }

    fn run(&self, name: &str) {
        let func = self.module.get(name).unwrap();
        let module: Module = Module::new();
        let v = func.value();
        let mut eval: Evaluator = Evaluator::new(&module);
        let result = eval.eval_function(v, &[], &[]).unwrap();
        println!("result: {}", result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let content = r#"
i = 1
def hello():
    return "hello" + str(i)
i += 21
"#;
        let s = Script::new(content).unwrap();
        s.run("hello");
        panic!("test")
    }
}
