use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, sqlx::Type, Deserialize, Serialize, Debug)]
#[sqlx(type_name = "promptkit.function_visibility", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum FunctionVisibility {
    Public,
    Internal,
    Private,
}

#[derive(Clone, PartialOrd, Copy, PartialEq, sqlx::Type, Deserialize, Serialize, Debug)]
#[sqlx(type_name = "promptkit.function_permission", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum FunctionPermission {
    Viewer,
    Editor,
    Owner,
}
