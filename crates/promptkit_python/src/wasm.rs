use crate::script::{InputValue, Script, VM};

use std::cell::RefCell;

wit_bindgen::generate!({
    world: "python-executor",
    exports: {
        "python-executor": Global,
        "python-executor/script": PyScript,
    },
});

pub struct Global();

impl exports::python_executor::Guest for Global {
    fn run_script(script: String, func: String, args: Vec<String>) -> Result<String, String> {
        GLOBAL_VM.with(|vm| {
            if let Some(vm) = vm.borrow().as_ref() {
                vm.load_script(&script).map_err(|e| e.to_string())?;
                let v = vm
                    .run(
                        &func,
                        args.into_iter()
                            .map(|f| InputValue::Json(serde_json::from_str(&f).unwrap())),
                        [],
                    )
                    .map_err(|e| e.to_string())?;
                return Ok(v.to_string());
            } else {
                return Err("VM not initialized".to_string());
            }
        })
    }
}

pub struct PyScript(VM);

impl exports::python_executor::GuestScript for PyScript {
    fn new() -> PyScript {
        PyScript(VM::new())
    }

    fn run(&self, script: String, func: String, args: Vec<String>) -> Result<String, String> {
        let v = self
            .0
            .script(&script)
            .map_err(|e| e.to_string())?
            .run(
                &func,
                args.into_iter()
                    .map(|f| InputValue::Json(serde_json::from_str(&f).unwrap())),
                [],
            )
            .map_err(|e| e.to_string())?;
        return Ok(v.to_string());
    }
}

thread_local! {
    static GLOBAL_VM: RefCell<Option<Script>> = RefCell::new(None);
}

#[export_name = "wizer.initialize"]
pub extern "C" fn init() {
    GLOBAL_VM.with(|vm| {
        let v = VM::new();
        let s = v.script("import json, re").unwrap();
        vm.borrow_mut().replace(s);
    });
}
