use std::cell::RefCell;

use self::exports::vm::Argument;
use crate::script::InputValue;
use crate::script::Scope;

wit_bindgen::generate!({
    world: "python-vm",
    exports: {
        "vm": Global,
    },
});

pub struct Global;

impl exports::vm::Guest for Global {
    fn eval_script(script: String) -> Result<(), exports::vm::Error> {
        GLOBAL_SCOPE.with(|vm| {
            return if let Some(vm) = vm.borrow().as_ref() {
                vm.load_script(&script)
                    .map_err(|e| exports::vm::Error::Python(e.to_string()))?;
                Ok(())
            } else {
                Err(exports::vm::Error::Unknown(
                    "VM not initialized".to_string(),
                ))
            };
        })
    }

    fn call_func(func: String, args: Vec<Argument>) -> Result<(), exports::vm::Error> {
        GLOBAL_SCOPE.with(|vm| {
            if let Some(vm) = vm.borrow().as_ref() {
                let ret = vm
                    .run(
                        &func,
                        args.iter().map(|f| match f {
                            Argument::Json(s) => InputValue::JsonStr(s),
                        }),
                        [],
                        |s| host::emit(s, false),
                    )
                    .map_err(|e| exports::vm::Error::Python(e.to_string()))?;
                host::emit(ret.as_deref().unwrap_or(""), true);
                Ok(())
            } else {
                Err(exports::vm::Error::Unknown(
                    "VM not initialized".to_string(),
                ))
            }
        })
    }
}

thread_local! {
    static GLOBAL_SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

#[export_name = "wizer.initialize"]
pub extern "C" fn _initialize() {
    extern "C" {
        fn __wasm_call_ctors();
    }
    unsafe { __wasm_call_ctors() };

    GLOBAL_SCOPE.with(|scope| {
        let v = Scope::new();
        v.load_script("").unwrap();
        scope.borrow_mut().replace(v);
    });
}
