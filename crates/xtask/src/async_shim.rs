use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
};

use anyhow::{Context, Result, bail};
use rustc_demangle::try_demangle;
use wasm_encoder::{
    CustomSection, Encode, EntityType, ExportKind, ExportSection, ImportSection, Module,
    RawSection, TypeSection, ValType,
};
use wasmparser::{Dylink0Subsection, Encoding, KnownCustom, Parser, Payload, TypeRef};
use wit_parser::WorldItem;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AsyncType {
    Future,
    Stream,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AsyncOperation {
    New,
    CancelWrite,
    CancelRead,
    DropWritable,
    DropReadable,
    StartRead,
    StartWrite,
}

impl AsyncOperation {
    const ALL: [Self; 7] = [
        Self::New,
        Self::CancelWrite,
        Self::CancelRead,
        Self::DropWritable,
        Self::DropReadable,
        Self::StartRead,
        Self::StartWrite,
    ];

    const fn rust_name(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::CancelWrite => "cancel_write",
            Self::CancelRead => "cancel_read",
            Self::DropWritable => "drop_writable",
            Self::DropReadable => "drop_readable",
            Self::StartRead => "start_read",
            Self::StartWrite => "start_write",
        }
    }

    fn from_rust_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.rust_name() == name)
    }

    const fn canonical_name(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::CancelWrite => "cancel-write",
            Self::CancelRead => "cancel-read",
            Self::DropWritable => "drop-writable",
            Self::DropReadable => "drop-readable",
            Self::StartRead => "read",
            Self::StartWrite => "write",
        }
    }

    const fn type_index(self, async_type: AsyncType) -> u32 {
        match self {
            Self::New => 0,
            Self::CancelWrite | Self::CancelRead => 1,
            Self::DropWritable | Self::DropReadable => 2,
            Self::StartRead | Self::StartWrite => match async_type {
                AsyncType::Future => 3,
                AsyncType::Stream => 4,
            },
        }
    }
}

#[derive(Debug)]
struct CanonicalVtable {
    module: String,
    ordinal: u32,
    function: String,
    function_order: usize,
}

fn parse_new_import(name: &str) -> Option<(AsyncType, u32, &str)> {
    let (async_type, rest) = name
        .strip_prefix("[future-new-")
        .map(|rest| (AsyncType::Future, rest))
        .or_else(|| {
            name.strip_prefix("[stream-new-")
                .map(|rest| (AsyncType::Stream, rest))
        })?;
    let (ordinal, function) = rest.split_once(']')?;
    Some((async_type, ordinal.parse().ok()?, function))
}

fn parse_vtable_symbol(name: &str) -> Option<(AsyncType, usize, AsyncOperation)> {
    let demangled = try_demangle(name).ok()?.to_string();
    parse_demangled_vtable_symbol(&demangled)
}

fn parse_demangled_vtable_symbol(demangled: &str) -> Option<(AsyncType, usize, AsyncOperation)> {
    let segments = demangled.split("::").collect::<Vec<_>>();

    segments.windows(3).find_map(|segments| {
        let async_type = match segments[0] {
            "wit_future" => AsyncType::Future,
            "wit_stream" => AsyncType::Stream,
            _ => return None,
        };
        let vtable = segments[1].strip_prefix("vtable")?.parse().ok()?;
        let operation = AsyncOperation::from_rust_name(segments[2])?;
        Some((async_type, vtable, operation))
    })
}

/// Return each WIT function's generation order. `wit-bindgen` emits async
/// vtables on the first encounter of a payload type, following this traversal.
fn canonical_function_order(runtime: &[u8]) -> Result<HashMap<(String, String), usize>> {
    let (_, bindgen) = wit_component::metadata::decode(runtime)
        .context("failed to decode the runtime's component metadata")?;
    let resolve = &bindgen.resolve;
    let world = &resolve.worlds[bindgen.world];
    let mut result = HashMap::new();
    let mut next = 0;

    let mut insert = |module: String, function: &str| -> Result<()> {
        if result
            .insert((module.clone(), function.to_string()), next)
            .is_some()
        {
            bail!("duplicate WIT function {module}/{function}");
        }
        next += 1;
        Ok(())
    };

    // Imports from interfaces are generated before freestanding imports.
    for (key, item) in &world.imports {
        if let WorldItem::Interface { id, .. } = item {
            let module = resolve.name_world_key(key);
            for function in resolve.interfaces[*id].functions.values() {
                insert(module.clone(), &function.name)?;
            }
        }
    }
    for item in world.imports.values() {
        if let WorldItem::Function(function) = item {
            insert("$root".to_string(), &function.name)?;
        }
    }

    // Freestanding exports are generated before exported interfaces.
    for item in world.exports.values() {
        if let WorldItem::Function(function) = item {
            insert("[export]$root".to_string(), &function.name)?;
        }
    }
    for (key, item) in &world.exports {
        if let WorldItem::Interface { id, .. } = item {
            let module = format!("[export]{}", resolve.name_world_key(key));
            for function in resolve.interfaces[*id].functions.values() {
                insert(module.clone(), &function.name)?;
            }
        }
    }

    Ok(result)
}

/// Build a shared-library shim for function pointers to canonical async
/// imports.
///
/// `wit-bindgen` places imported future/stream functions in static vtables. In
/// a Wasm shared object those addresses become `GOT.func` imports named after
/// the Rust declarations, while the canonical imports retain their WIT names.
/// The component linker cannot infer that they are the same function. This
/// module re-exports the canonical imports under the requested Rust symbol
/// names.
fn async_import_shim(runtime: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut canonical = BTreeMap::<AsyncType, Vec<CanonicalVtable>>::new();
    let mut symbols = BTreeMap::<(AsyncType, usize, AsyncOperation), String>::new();

    for payload in Parser::new(0).parse_all(runtime) {
        if let Payload::ImportSection(imports) = payload? {
            for import in imports.into_imports() {
                let import = import?;
                match import.ty {
                    TypeRef::Func(_) => {
                        if let Some((async_type, ordinal, function)) = parse_new_import(import.name)
                        {
                            canonical
                                .entry(async_type)
                                .or_default()
                                .push(CanonicalVtable {
                                    module: import.module.to_string(),
                                    ordinal,
                                    function: function.to_string(),
                                    function_order: 0,
                                });
                        }
                    }
                    TypeRef::Global(_) if import.module == "GOT.func" => {
                        if let Some((async_type, vtable, operation)) =
                            parse_vtable_symbol(import.name)
                            && symbols
                                .insert((async_type, vtable, operation), import.name.to_string())
                                .is_some()
                        {
                            bail!(
                                "duplicate {async_type:?} vtable {vtable} {} symbol",
                                operation.rust_name()
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if symbols.is_empty() {
        return Ok(None);
    }

    let function_order = canonical_function_order(runtime)?;
    for vtables in canonical.values_mut() {
        for vtable in vtables.iter_mut() {
            vtable.function_order = *function_order
                .get(&(vtable.module.clone(), vtable.function.clone()))
                .with_context(|| {
                    format!(
                        "canonical async import refers to unknown WIT function {}/{}",
                        vtable.module, vtable.function
                    )
                })?;
        }
        vtables.sort_by_key(|vtable| (vtable.function_order, vtable.ordinal));
    }

    for (&async_type, vtables) in &canonical {
        for vtable in 0..vtables.len() {
            for operation in AsyncOperation::ALL {
                if !symbols.contains_key(&(async_type, vtable, operation)) {
                    bail!(
                        "missing {async_type:?} vtable {vtable} {} symbol",
                        operation.rust_name()
                    );
                }
            }
        }
    }

    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I64]);
    types.ty().function([ValType::I32], [ValType::I32]);
    types.ty().function([ValType::I32], []);
    types
        .ty()
        .function([ValType::I32, ValType::I32], [ValType::I32]);
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]);
    types.ty().function(
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32],
    );
    module.section(&types);

    let mut imports = ImportSection::new();
    let mut exports = ExportSection::new();
    let mut function_index = 0;
    for ((async_type, vtable_index, operation), symbol) in symbols {
        let vtable = canonical
            .get(&async_type)
            .and_then(|vtables| vtables.get(vtable_index))
            .with_context(|| {
                format!("no canonical {async_type:?} import for vtable {vtable_index} ({symbol})")
            })?;
        let kind = match async_type {
            AsyncType::Future => "future",
            AsyncType::Stream => "stream",
        };
        let prefix = if matches!(
            operation,
            AsyncOperation::StartRead | AsyncOperation::StartWrite
        ) {
            "[async-lower]"
        } else {
            ""
        };
        let name = format!(
            "{prefix}[{kind}-{}-{}]{}",
            operation.canonical_name(),
            vtable.ordinal,
            vtable.function
        );
        imports.import(
            &vtable.module,
            &name,
            EntityType::Function(operation.type_index(async_type)),
        );
        exports.export(&symbol, ExportKind::Func, function_index);
        function_index += 1;
    }
    imports.import("env", "cabi_realloc", EntityType::Function(5));
    exports.export("cabi_realloc", ExportKind::Func, function_index);
    module.section(&imports);
    module.section(&exports);
    Ok(Some(module.finish()))
}

fn push_dylink_subsection(output: &mut Vec<u8>, id: u8, payload: Vec<u8>) {
    output.push(id);
    payload.encode(output);
}

fn encode_needed(names: &[&str]) -> Vec<u8> {
    let mut payload = Vec::new();
    u32::try_from(names.len()).unwrap().encode(&mut payload);
    for name in names {
        name.encode(&mut payload);
    }
    payload
}

fn dylink_with_needed(
    section: wasmparser::CustomSectionReader<'_>,
    library: &str,
) -> Result<Vec<u8>> {
    let KnownCustom::Dylink0(mut subsections) = section.as_known() else {
        unreachable!();
    };
    let source = section.data();
    let source_offset = section.data_offset();
    let mut output = Vec::new();
    let mut found_needed = false;

    loop {
        let start = subsections.original_position() - source_offset;
        let Some(subsection) = subsections.next() else {
            break;
        };
        let end = subsections.original_position() - source_offset;
        match subsection? {
            Dylink0Subsection::Needed(mut names) => {
                if found_needed {
                    bail!("runtime contains multiple dylink.0 needed subsections");
                }
                found_needed = true;
                if !names.contains(&library) {
                    names.push(library);
                }
                push_dylink_subsection(&mut output, 2, encode_needed(&names));
            }
            _ => {
                output.extend_from_slice(&source[start..end]);
            }
        }
    }

    if !found_needed {
        push_dylink_subsection(&mut output, 2, encode_needed(&[library]));
    }
    Ok(output)
}

fn add_needed_library(runtime: &[u8], library: &str) -> Result<Vec<u8>> {
    let mut module = Module::new();
    let mut found_dylink = false;

    for payload in Parser::new(0).parse_all(runtime) {
        match payload? {
            Payload::CustomSection(section) if section.name() == "dylink.0" => {
                if found_dylink {
                    bail!("runtime contains multiple dylink.0 sections");
                }
                found_dylink = true;
                module.section(&CustomSection {
                    name: Cow::Borrowed("dylink.0"),
                    data: Cow::Owned(dylink_with_needed(section, library)?),
                });
            }
            Payload::Version { encoding, .. } if encoding != Encoding::Module => {
                bail!("runtime is not a core Wasm module");
            }
            payload => {
                if let Some((id, range)) = payload.as_section() {
                    module.section(&RawSection {
                        id,
                        data: &runtime[range],
                    });
                }
            }
        }
    }

    if !found_dylink {
        bail!("runtime has no dylink.0 section");
    }
    Ok(module.finish())
}

pub(super) fn link_library(
    linker: wit_component::Linker,
    name: &str,
    data: &[u8],
    dl_openable: bool,
    async_shim_name: Option<&str>,
) -> Result<wit_component::Linker> {
    let Some(shim_name) = async_shim_name else {
        return linker.library(name, data, dl_openable);
    };
    let Some(shim) = async_import_shim(data)? else {
        return linker.library(name, data, dl_openable);
    };

    let runtime = add_needed_library(data, shim_name)?;
    linker
        .library(name, &runtime, dl_openable)?
        .library(shim_name, &shim, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multidigit_vtable_symbols() {
        assert_eq!(
            parse_demangled_vtable_symbol("isola_runtime::wasm::wit_future::vtable12::start_write"),
            Some((AsyncType::Future, 12, AsyncOperation::StartWrite))
        );
        assert_eq!(
            parse_demangled_vtable_symbol("isola_runtime::wasm::wit_stream::vtable3::new"),
            Some((AsyncType::Stream, 3, AsyncOperation::New))
        );
    }

    #[test]
    fn adds_a_dylink_dependency_once() -> Result<()> {
        let mut memory_info = Vec::new();
        for value in [1_u32, 2, 3, 4] {
            value.encode(&mut memory_info);
        }
        let mut dylink = Vec::new();
        push_dylink_subsection(&mut dylink, 1, memory_info);
        push_dylink_subsection(&mut dylink, 2, encode_needed(&["libc.so"]));

        let mut module = Module::new();
        module.section(&CustomSection {
            name: Cow::Borrowed("dylink.0"),
            data: Cow::Owned(dylink),
        });
        let updated = add_needed_library(&module.finish(), "libasync.so")?;
        let updated = add_needed_library(&updated, "libasync.so")?;

        for payload in Parser::new(0).parse_all(&updated) {
            if let Payload::CustomSection(section) = payload?
                && let KnownCustom::Dylink0(subsections) = section.as_known()
            {
                for subsection in subsections {
                    if let Dylink0Subsection::Needed(names) = subsection? {
                        assert_eq!(names, ["libc.so", "libasync.so"]);
                        return Ok(());
                    }
                }
            }
        }
        bail!("updated module has no dylink.0 needed subsection")
    }
}
