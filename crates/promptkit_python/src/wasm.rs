use crate::script::{InputValue, Script, VM};

use std::cell::RefCell;

wit_bindgen::generate!({
    world: "python-vm",
    exports: {
        "python-vm": Global,
    },
});

pub struct Global();

impl exports::python_vm::Guest for Global {
    fn eval_script(script: String) -> Result<(), String> {
        GLOBAL_VM.with(|vm| {
            if let Some(vm) = vm.borrow().as_ref() {
                vm.load_script(&script).map_err(|e| e.to_string())?;
                return Ok(());
            } else {
                return Err("VM not initialized".to_string());
            }
        })
    }

    fn call_func(func: String, args: Vec<String>) -> Result<(), String> {
        GLOBAL_VM.with(|vm| {
            if let Some(vm) = vm.borrow().as_ref() {
                let ret = vm
                    .run(
                        &func,
                        args.iter().map(|f| InputValue::JsonStr(f)),
                        [],
                        |s| host::emit(s, false),
                    )
                    .map_err(|e| e.to_string())?;
                if let Some(ret) = ret {
                    host::emit(&ret, true);
                } else {
                    host::emit("", true);
                }
                return Ok(());
            } else {
                return Err("VM not initialized".to_string());
            }
        })
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
