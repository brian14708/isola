use std::path::Path;

use oxc::{
    allocator::Allocator,
    codegen::Codegen,
    parser::Parser,
    semantic::SemanticBuilder,
    span::SourceType,
    transformer::{TransformOptions, Transformer},
};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("TypeScript transpilation failed: {message}")]
pub struct TranspileError {
    message: String,
}

impl TranspileError {
    fn from_diagnostics(
        source_name: &Path,
        source_text: &str,
        errors: Vec<oxc::diagnostics::OxcDiagnostic>,
    ) -> Self {
        let rendered = errors
            .into_iter()
            .map(|error| format!("{:?}", error.with_source_code(source_text.to_owned())))
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            message: format!("{}:\n{rendered}", source_name.display()),
        }
    }
}

pub fn strip_typescript(
    source_text: &str,
    source_name: Option<&Path>,
) -> Result<String, TranspileError> {
    let source_name = source_name.unwrap_or_else(|| Path::new("<guest>"));
    let allocator = Allocator::default();
    let source_type = SourceType::default()
        .with_script(true)
        .with_typescript(true);

    let parser = Parser::new(&allocator, source_text, source_type).parse();
    if !parser.errors.is_empty() {
        return Err(TranspileError::from_diagnostics(
            source_name,
            source_text,
            parser.errors,
        ));
    }

    let mut program = parser.program;
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .with_excess_capacity(2.0)
        .build(&program);
    if !semantic.errors.is_empty() {
        return Err(TranspileError::from_diagnostics(
            source_name,
            source_text,
            semantic.errors,
        ));
    }

    let scoping = semantic.semantic.into_scoping();
    let transformed = Transformer::new(&allocator, source_name, &TransformOptions::default())
        .build_with_scoping(scoping, &mut program);
    if !transformed.errors.is_empty() {
        return Err(TranspileError::from_diagnostics(
            source_name,
            source_text,
            transformed.errors,
        ));
    }

    Ok(Codegen::new().build(&program).code)
}

#[cfg(test)]
mod tests {
    use super::strip_typescript;

    #[test]
    fn strips_type_annotations() {
        let output = strip_typescript(
            "function add(a: number, b: number): number { return a + b; }",
            None,
        )
        .expect("typescript should transpile");
        assert!(output.contains("function add(a, b)"));
        assert!(!output.contains(": number"));
    }

    #[test]
    fn accepts_plain_javascript() {
        let output = strip_typescript("function main() { return 42; }", None)
            .expect("plain javascript should transpile");
        assert!(output.contains("function main()"));
        assert!(output.contains("return 42;"));
    }

    #[test]
    fn rejects_invalid_typescript() {
        let err = strip_typescript("function main(: number) {}", None)
            .expect_err("invalid typescript should fail");
        assert!(err.to_string().contains("TypeScript transpilation failed"));
    }
}
