use serde::{Deserialize, Serialize};

use crate::proto::script::v1 as script;

#[derive(Serialize)]
pub struct AnalyzeRequest<'a> {
    pub methods: Vec<&'a str>,
}

#[derive(Deserialize)]
pub struct Type {
    pub name: Option<String>,
    pub json_schema: Option<String>,
}

#[derive(Deserialize)]
pub struct Method {
    pub name: String,
    pub description: Option<String>,
    pub argument_types: Vec<Type>,
    pub result_type: Type,
}

#[derive(Deserialize)]
pub struct AnalyzeResult {
    pub method_infos: Vec<Method>,
}

impl From<AnalyzeResult> for script::AnalyzeResult {
    fn from(val: AnalyzeResult) -> Self {
        Self {
            method_infos: val
                .method_infos
                .into_iter()
                .map(|m| script::MethodInfo {
                    name: m.name,
                    description: m.description.unwrap_or_default(),
                    argument_types: m
                        .argument_types
                        .into_iter()
                        .map(|a| script::TypeInfo {
                            name: a.name.unwrap_or_default(),
                            json_schema: a.json_schema.unwrap_or_default(),
                        })
                        .collect(),
                    result_type: Some(script::TypeInfo {
                        name: m.result_type.name.unwrap_or_default(),
                        json_schema: m.result_type.json_schema.unwrap_or_default(),
                    }),
                })
                .collect(),
        }
    }
}

impl<'a> From<&'a script::AnalyzeRequest> for AnalyzeRequest<'a> {
    fn from(req: &'a script::AnalyzeRequest) -> Self {
        Self {
            methods: req.methods.iter().map(String::as_str).collect(),
        }
    }
}
